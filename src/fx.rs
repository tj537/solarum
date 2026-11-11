//! The send effects: echo, reverb and the kick's rumble.

use crate::synth::{sat, Svf};
use std::f32::consts::TAU;

/// A delay line read at a fractional distance behind the write position
struct Line {
    buf: Vec<f32>,
    at: usize,
}

impl Line {
    fn new(len: usize) -> Self {
        Line {
            buf: vec![0.0; len.max(2)],
            at: 0,
        }
    }

    /// The sample written `back` samples ago (at least 1, at most the line's length)
    fn read(&self, back: f32) -> f32 {
        let n = self.buf.len();
        let back = back.clamp(1.0, (n - 1) as f32);
        let whole = back as usize;
        let frac = back - whole as f32;
        let i = (self.at + n - whole) % n;
        let j = (i + n - 1) % n;
        self.buf[i] + (self.buf[j] - self.buf[i]) * frac
    }

    fn write(&mut self, x: f32) {
        self.buf[self.at] = x;
        self.at = (self.at + 1) % self.buf.len();
    }
}

/// Stereo ping-pong echo. Every repeat comes back darker and thinner (a low-pass and a
/// high-pass in the feedback), so a long tail fades into the background instead of piling
/// up in the low mids. The time glides, so a tempo change bends the echoes like a tape
/// instead of clicking.
pub struct Delay {
    left: Line,
    right: Line,
    time: f32,
    glide: f32,
    dark: [Svf; 2],
    thin: [Svf; 2],
}

impl Delay {
    pub fn new(sr: f32) -> Self {
        let n = (sr * 2.0) as usize;
        Delay {
            left: Line::new(n),
            right: Line::new(n),
            time: 0.0,
            glide: 1.0 - (-1.0 / (0.08 * sr)).exp(),
            dark: [Svf::new(3_200.0, 0.6, sr), Svf::new(3_200.0, 0.6, sr)],
            thin: [Svf::new(260.0, 0.6, sr), Svf::new(260.0, 0.6, sr)],
        }
    }

    /// `time` in samples; returns the wet signal
    pub fn run(&mut self, l: f32, r: f32, time: f32, feedback: f32) -> (f32, f32) {
        // The first time is taken as it is – only changes glide
        self.time = if self.time == 0.0 {
            time
        } else {
            self.time + (time - self.time) * self.glide
        };
        let (dl, dr) = (self.left.read(self.time), self.right.read(self.time));
        // Ping-pong: left feeds right and vice versa
        let fl = self.thin[0].run(self.dark[0].run(dr).lp).hp;
        let fr = self.thin[1].run(self.dark[1].run(dl).lp).hp;
        self.left.write(l + sat(fl * feedback));
        self.right.write(r * 0.3 + sat(fr * feedback));
        (dl, dr)
    }
}

/// An all-pass that smears a click into a little cloud without colouring it
struct Diffuser {
    line: Line,
    len: f32,
}

impl Diffuser {
    fn run(&mut self, x: f32) -> f32 {
        const G: f32 = 0.6;
        let back = self.line.read(self.len);
        let into = x + back * G;
        self.line.write(into);
        back - into * G
    }
}

/// The reverb's delay lines in milliseconds: none a simple fraction of another, so their
/// echoes never fall on each other
const ROOM_MS: [f32; 8] = [31.7, 37.1, 41.3, 47.9, 53.9, 61.7, 67.3, 73.1];
const DIFFUSE_MS: [f32; 4] = [4.77, 3.59, 12.73, 9.31];
/// The room answers this long after the sound, so the hit itself stays dry and clear
const PRE_DELAY: f32 = 0.012;
/// Nothing below this goes into the room – a kick in a reverb is mud
const ROOM_LOW_CUT: f32 = 220.0;

/// The room: a feedback delay network. The input is cut below `ROOM_LOW_CUT`, delayed a
/// little and smeared by four all-passes; eight delay lines then feed each other through a
/// mixing matrix (Householder), so the echoes multiply into a smooth tail instead of the
/// metallic ring of parallel combs. Two of the lines breathe slowly, which keeps single
/// resonances from standing out.
pub struct Reverb {
    sr: f32,
    low_cut: Svf,
    pre: Line,
    diffusers: [Diffuser; 4],
    lines: [Line; 8],
    /// One-pole low-pass per line: highs die sooner, like in a real room
    damped: [f32; 8],
    lfo: f32,
}

impl Reverb {
    pub fn new(sr: f32) -> Self {
        let samples = |ms: f32| ms * 0.001 * sr;
        Reverb {
            sr,
            low_cut: Svf::new(ROOM_LOW_CUT, 0.6, sr),
            pre: Line::new((PRE_DELAY * sr) as usize + 2),
            diffusers: DIFFUSE_MS.map(|ms| Diffuser {
                line: Line::new(samples(ms) as usize + 2),
                len: samples(ms),
            }),
            // Room for the slow breathing on top of the length
            lines: ROOM_MS.map(|ms| Line::new(samples(ms + 2.0) as usize + 2)),
            damped: [0.0; 8],
            lfo: 0.0,
        }
    }

    /// Seconds until the tail has fallen by 60 dB, for a `size` of 0…1
    pub fn seconds(size: f32) -> f32 {
        0.7 * (5.0 * (size - 0.5)).exp()
    }

    /// `size` 0…1 (how long it rings, see `seconds`), `damp` 0…1 (how dark the tail is)
    pub fn run(&mut self, input: f32, size: f32, damp: f32) -> (f32, f32) {
        let sr = self.sr;
        let mut x = self.low_cut.run(input).hp;
        self.pre.write(x);
        x = self.pre.read(PRE_DELAY * sr);
        for d in &mut self.diffusers {
            x = d.run(x);
        }

        self.lfo = (self.lfo + 0.37 / sr).fract();
        let wobble = (self.lfo * TAU).sin() * 0.0004 * sr;
        let mut out = [0.0f32; 8];
        for (i, line) in self.lines.iter().enumerate() {
            let len = ROOM_MS[i] * 0.001 * sr;
            out[i] = line.read(match i {
                2 => len + wobble,
                5 => len - wobble,
                _ => len,
            });
        }
        // Householder: every line hears itself less the sum of all – energy stays, echoes spread
        let mix = out.iter().sum::<f32>() * 0.25;
        let decay = -3.0 / (Self::seconds(size) * 1000.0);
        let damp = damp.clamp(0.0, 0.95);
        for (i, line) in self.lines.iter_mut().enumerate() {
            let gain = 10f32.powf(decay * ROOM_MS[i]);
            self.damped[i] = (out[i] - mix) * (1.0 - damp) + self.damped[i] * damp;
            line.write(x + self.damped[i] * gain);
        }
        // Left and right hear different lines in different phase: wide, and still mono-safe
        let l = out[0] - out[2] + out[4] - out[6];
        let r = out[1] - out[3] + out[5] - out[7];
        (l * ROOM_LEVEL, r * ROOM_LEVEL)
    }
}

/// Sets the room's level against the dry sound
const ROOM_LEVEL: f32 = 0.55;

/// The techno rumble: the kick smeared through dark resonating delays,
/// ducked whenever the kick hits
pub struct Rumble {
    lines: Vec<(Vec<f32>, usize)>,
    lp: Svf,
    out_lp: Svf,
}

impl Rumble {
    pub fn new(sr: f32) -> Self {
        let lines = [0.027f32, 0.039, 0.053, 0.071]
            .iter()
            .map(|s| (vec![0.0; (s * sr) as usize], 0))
            .collect();
        Rumble {
            lines,
            lp: Svf::new(160.0, 0.7, sr),
            out_lp: Svf::new(110.0, 0.8, sr),
        }
    }

    pub fn run(&mut self, kick: f32, duck: f32) -> f32 {
        let x = self.lp.run(kick).lp;
        let mut sum = 0.0;
        for (buf, pos) in self.lines.iter_mut() {
            let y = buf[*pos];
            buf[*pos] = x + y * 0.8;
            *pos = (*pos + 1) % buf.len();
            sum += y;
        }
        let d = (1.0 - duck * 1.5).max(0.0);
        self.out_lp.run(sat(sum * 0.5)).lp * d * d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listen;
    use crate::rng::Rng;

    const SR: f32 = 44_100.0;

    /// What the room answers to one click, `seconds` long (interleaved stereo)
    fn click_in_the_room(size: f32, damp: f32, seconds: f32) -> Vec<f32> {
        let mut room = Reverb::new(SR);
        (0..(seconds * SR) as usize)
            .flat_map(|i| {
                let (l, r) = room.run(if i == 0 { 1.0 } else { 0.0 }, size, damp);
                [l, r]
            })
            .collect()
    }

    fn level_at(tail: &[f32], seconds: f32) -> f32 {
        let at = (seconds * SR) as usize * 2;
        listen::db(listen::rms(&tail[at..at + 4410]))
    }

    #[test]
    fn the_room_rings_as_long_as_it_says() {
        for size in [0.5, 0.66, 0.9] {
            let rt = Reverb::seconds(size);
            let tail = click_in_the_room(size, 0.0, rt * 1.2 + 0.3);
            assert!(tail.iter().all(|x| x.is_finite()));
            // Measured between 0.1 s and the end: the slope says how long 60 dB take
            let (a, b) = (0.1, rt);
            let fall = level_at(&tail, a) - level_at(&tail, b);
            let measured = 60.0 * (b - a) / fall;
            assert!(
                (measured / rt - 1.0).abs() < 0.25,
                "size {size}: {measured:.2} s instead of {rt:.2} s"
            );
        }
    }

    #[test]
    fn the_tail_is_a_smooth_cloud_not_a_row_of_echoes() {
        // Kurtosis of the tail: 3 is noise-like (smooth), single echoes standing out push it
        // far up – the old parallel combs measured about twice this
        let tail = listen::mono(&click_in_the_room(0.66, 0.3, 1.0));
        let part = &tail[(0.08 * SR) as usize..(0.4 * SR) as usize];
        // Level out the decay first, in short blocks
        let flat: Vec<f32> = part
            .chunks(256)
            .flat_map(|c| {
                let r = listen::rms(c).max(1e-12);
                c.iter().map(move |x| x / r)
            })
            .collect();
        let n = flat.len() as f32;
        let m2 = flat.iter().map(|x| x * x).sum::<f32>() / n;
        let m4 = flat.iter().map(|x| x.powi(4)).sum::<f32>() / n;
        let kurtosis = m4 / (m2 * m2);
        assert!(kurtosis < 4.0, "kurtosis {kurtosis:.2}");
    }

    #[test]
    fn the_room_is_wide_dry_at_first_and_keeps_the_kick_out() {
        let tail = click_in_the_room(0.66, 0.3, 1.0);
        let h = listen::hear(&tail[..SR as usize], SR);
        assert!(h.width.abs() < 0.3, "left and right differ: {}", h.width);
        assert!(h.dc < 1e-3);
        // Nothing for the first milliseconds: the hit stays dry
        assert!(listen::peak(&tail[..(PRE_DELAY * SR) as usize * 2 - 4]) < 1e-6);
        // A low thump barely gets in, a clap does
        let ring = |hz: f32| {
            let mut room = Reverb::new(SR);
            let out: Vec<f32> = (0..SR as usize)
                .map(|i| room.run((TAU * hz * i as f32 / SR).sin(), 0.66, 0.3).0)
                .collect();
            listen::db(listen::rms(&out[SR as usize / 2..]))
        };
        assert!(ring(1_500.0) > ring(50.0) + 18.0, "{:.1} vs {:.1} dB", ring(1_500.0), ring(50.0));
    }

    #[test]
    fn a_dark_room_loses_its_highs_first() {
        let bright = |damp: f32| {
            let tail = listen::mono(&click_in_the_room(0.8, damp, 1.5));
            listen::centroid(&listen::spectrum(&tail[SR as usize / 2..], 4096), SR)
        };
        assert!(bright(0.3) > bright(0.8) * 1.5, "{:.0} vs {:.0} Hz", bright(0.3), bright(0.8));
    }

    #[test]
    fn echoes_come_back_on_time_darker_and_never_pile_up() {
        let mut echo = Delay::new(SR);
        let time = 0.25 * SR;
        let mut rng = Rng::new(4);
        // A short burst of noise, then five seconds of nothing but echoes at the highest feedback
        let out: Vec<f32> = (0..(5.0 * SR) as usize)
            .flat_map(|i| {
                let x = if i < 400 { rng.noise() } else { 0.0 };
                let (l, r) = echo.run(x, x, time, 0.85);
                [l, r]
            })
            .collect();
        assert!(out.iter().all(|x| x.is_finite()) && listen::peak(&out) <= 1.0);
        let repeat = |k: usize| {
            let at = k * time as usize * 2;
            &out[at..at + 1600]
        };
        // Silent until the first repeat, which is the burst itself
        assert!(listen::peak(&out[..time as usize * 2 - 2]) < 1e-6);
        assert!(listen::rms(repeat(1)) > 0.2);
        // Every repeat is quieter and darker than the one before
        let bright = |b: &[f32]| listen::centroid(&listen::spectrum(&listen::mono(b), 512), SR);
        for k in 1..6 {
            assert!(listen::rms(repeat(k + 1)) < listen::rms(repeat(k)), "repeat {k}");
        }
        assert!(bright(repeat(6)) < bright(repeat(1)) * 0.6);
    }

    #[test]
    fn a_tempo_change_bends_the_echo_instead_of_clicking() {
        let mut echo = Delay::new(SR);
        let mut biggest_step = 0.0f32;
        let mut last = 0.0;
        for i in 0..(2.0 * SR) as usize {
            // A steady low tone going in; the time jumps halfway through
            let x = (TAU * 110.0 * i as f32 / SR).sin() * 0.5;
            let time = if i < SR as usize { 9_000.0 } else { 7_000.0 };
            let (l, _) = echo.run(x, x, time, 0.0);
            if i > 10_000 {
                biggest_step = biggest_step.max((l - last).abs());
            }
            last = l;
        }
        // A 110 Hz tone at 0.5 moves 0.008 per sample; a jump in the line would be a step of up to 1
        assert!(biggest_step < 0.03, "{biggest_step}");
    }
}
