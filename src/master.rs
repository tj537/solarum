//! The master bus: the last thing every sound goes through.
//!
//! The mix arrives with headroom and leaves loud but clean: a DC blocker, the DRIVE knob's
//! saturation (the lows first, the whole mix only from "hot" on), a look-ahead limiter that turns
//! the level down *before* a peak arrives, and a soft clipper that rounds off the last
//! decibel of a transient. Below the clipper's knee the path is linear, so a quiet hat stays
//! a hat while the kick hits – one waveshaper over the whole mix (as it used to be) made
//! every sound distort every other.

use crate::synth::Svf;
use std::f32::consts::TAU;

/// Nothing leaves the master above this (before the volume)
pub const CEILING: f32 = 0.97;
/// How far above the ceiling the limiter lets a peak through – the clipper rounds that off
const CLIP_RANGE: f32 = 1.12;
/// The limiter looks this far ahead (and delays the output by as much)
const LOOKAHEAD: f32 = 0.004;
/// How fast the limiter lets go again
const RELEASE: f32 = 0.12;

/// Removes a constant offset (saturation of lopsided waves leaves one) without touching the sub
#[derive(Clone, Copy, Default)]
struct DcBlock {
    x: f32,
    y: f32,
}

impl DcBlock {
    fn run(&mut self, x: f32, r: f32) -> f32 {
        self.y = x - self.x + r * self.y;
        self.x = x;
        self.y
    }
}

/// `tanh` pushed by `gain`, levelled so that a signal at half scale stays as loud: it adds
/// colour, not volume
#[derive(Clone, Copy)]
struct Shaper {
    gain: f32,
    level: f32,
}

impl Shaper {
    fn new(gain: f32) -> Self {
        Shaper {
            gain,
            level: 0.5 / (0.5 * gain).tanh(),
        }
    }

    fn run(&self, x: f32) -> f32 {
        (x * self.gain).tanh() * self.level
    }
}

/// The DRIVE knob. Up to "warm" it only saturates the lows (below `DRIVE_SPLIT`): kick and
/// bass grow overtones – they get heard on small speakers – while everything above passes
/// untouched. From "hot" on, the whole mix is pushed into the shaper as well.
#[derive(Clone, Copy)]
pub struct Drive {
    low: Shaper,
    full: Shaper,
}

/// Below this the DRIVE knob saturates first (Hz)
const DRIVE_SPLIT: f32 = 180.0;

impl Drive {
    /// `amount` 0…1: clean · warm · hot · fuzz
    pub fn new(amount: f32) -> Self {
        let a = amount.clamp(0.0, 1.0);
        let over = ((a - 0.35) / 0.65).max(0.0);
        Drive {
            low: Shaper::new(0.1 + 3.4 * a.powf(1.2)),
            full: Shaper::new(0.1 + 5.9 * over.powf(1.7)),
        }
    }
}

/// Linear up to a knee, then a parabola that lands flat on the ceiling at `CLIP_RANGE` times it
pub fn soft_clip(x: f32) -> f32 {
    let top = CEILING * CLIP_RANGE;
    let knee = 2.0 * CEILING - top;
    let a = x.abs();
    if a <= knee {
        x
    } else if a >= top {
        CEILING.copysign(x)
    } else {
        (CEILING - (top - a) * (top - a) / (2.0 * (top - knee))).copysign(x)
    }
}

/// A look-ahead limiter. Every sample says what gain it needs to stay under the threshold;
/// the gain that plays is the lowest of those over the look-ahead (so it is down before the
/// peak arrives), released slowly and smoothed with a moving average (so it never steps).
pub struct Limiter {
    threshold: f32,
    /// The audio on its way out, `line.len()` frames late
    line: Vec<[f32; 2]>,
    at: usize,
    /// Sliding minimum of the wanted gains: (time, gain), gains rising from front to back
    lows: Vec<(u64, f32)>,
    front: usize,
    held: usize,
    now: u64,
    /// The minimum with its slow release
    gain: f32,
    release: f32,
    /// The last `line.len()` released gains and their sum – the moving average
    gains: Vec<f32>,
    sum: f64,
}

impl Limiter {
    pub fn new(threshold: f32, sr: f32) -> Self {
        let n = ((LOOKAHEAD * sr) as usize).max(1);
        Limiter {
            threshold,
            line: vec![[0.0; 2]; n],
            at: 0,
            lows: vec![(0, 1.0); n + 1],
            front: 0,
            held: 0,
            now: 0,
            gain: 1.0,
            release: 1.0 - (-1.0 / (RELEASE * sr)).exp(),
            gains: vec![1.0; n],
            sum: n as f64,
        }
    }

    /// Frames between a sample going in and coming out
    #[cfg(test)]
    pub fn latency(&self) -> usize {
        self.line.len()
    }

    /// How far the level is turned down right now (1 = not at all)
    #[cfg(test)]
    pub fn gain(&self) -> f32 {
        (self.sum / self.gains.len() as f64) as f32
    }

    pub fn run(&mut self, l: f32, r: f32) -> (f32, f32) {
        let n = self.line.len();
        let cap = self.lows.len();
        let peak = l.abs().max(r.abs());
        let want = if peak > self.threshold { self.threshold / peak } else { 1.0 };

        // The lowest wanted gain of the last n + 1 samples: drop what the new one undercuts
        // from the back, what is too old from the front
        while self.held > 0 && self.lows[(self.front + self.held - 1) % cap].1 >= want {
            self.held -= 1;
        }
        self.lows[(self.front + self.held) % cap] = (self.now, want);
        self.held += 1;
        while self.lows[self.front].0 + (n as u64) < self.now {
            self.front = (self.front + 1) % cap;
            self.held -= 1;
        }
        let low = self.lows[self.front].1;
        self.now += 1;

        self.gain = if low < self.gain {
            low
        } else {
            self.gain + (low - self.gain) * self.release
        };
        self.sum += (self.gain - self.gains[self.at]) as f64;
        self.gains[self.at] = self.gain;
        let g = (self.sum / n as f64) as f32;

        let [dl, dr] = std::mem::replace(&mut self.line[self.at], [l, r]);
        self.at = (self.at + 1) % n;
        (dl * g, dr * g)
    }
}

pub struct Master {
    dc: [DcBlock; 2],
    dc_coef: f32,
    /// Splits off the lows for the DRIVE knob
    split: [Svf; 2],
    limiter: Limiter,
}

impl Master {
    pub fn new(sr: f32) -> Self {
        Master {
            dc: Default::default(),
            dc_coef: 1.0 - TAU * 12.0 / sr,
            split: [Svf::new(DRIVE_SPLIT, 0.5, sr), Svf::new(DRIVE_SPLIT, 0.5, sr)],
            limiter: Limiter::new(CEILING * CLIP_RANGE, sr),
        }
    }

    /// One frame through the master
    pub fn run(&mut self, l: f32, r: f32, drive: Drive, volume: f32) -> (f32, f32) {
        let mut frame = [l, r];
        for (ch, x) in frame.iter_mut().enumerate() {
            let dry = self.dc[ch].run(*x, self.dc_coef);
            // The lows and the rest add up to the input again, so "clean" is clean
            let low = self.split[ch].run(dry).lp;
            *x = drive.full.run(drive.low.run(low) + (dry - low));
        }
        let (l, r) = self.limiter.run(frame[0], frame[1]);
        (soft_clip(l) * volume, soft_clip(r) * volume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listen;

    const SR: f32 = 44_100.0;

    fn through(master: &mut Master, input: &[f32], drive: f32) -> Vec<f32> {
        let drive = Drive::new(drive);
        input
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|f| {
                let (l, r) = master.run(f[0], f[1], drive, 1.0);
                [l, r]
            })
            .collect()
    }

    #[test]
    fn nothing_gets_past_the_ceiling() {
        // Far too loud, with a sudden start: never a sample above the ceiling
        for amp in [0.5, 1.0, 3.0, 20.0] {
            let mut m = Master::new(SR);
            let mut loud = listen::sine(55.0, amp, 0.5, SR);
            loud.extend(listen::sine(3_100.0, amp, 0.5, SR));
            let out = through(&mut m, &loud, 0.0);
            assert!(listen::peak(&out) <= CEILING + 1e-6, "{amp}: {}", listen::peak(&out));
        }
    }

    #[test]
    fn a_mix_with_headroom_passes_untouched() {
        // Under the clipper's knee and with DRIVE at clean, what comes out is what went in
        let mut m = Master::new(SR);
        let tone = listen::sine(440.0, 0.5, 1.0, SR);
        let out = listen::mono(&through(&mut m, &tone, 0.0));
        let dirt = listen::dirt_db(&out[4096..], SR, &[440.0], 30.0);
        assert!(dirt < -60.0, "{dirt:.1} dB of dirt");
        let level = listen::db(listen::rms(&out[4096..])) - listen::db(listen::rms(&listen::mono(&tone)));
        assert!(level.abs() < 0.2, "{level:.2} dB");
    }

    /// A quiet 7 kHz "hat" over a 50 Hz thump four times a second, and how much of what
    /// lands around the hat isn't the hat (in dB below it): the thump's fingerprints on it
    fn hat_dirt(thump: f32, drive: f32) -> f32 {
        let n = SR as usize * 2;
        let input: Vec<f32> = (0..n)
            .flat_map(|i| {
                let t = i as f32 / SR;
                let beat = (t * 4.0).fract() / 4.0;
                let env = (beat / 0.003).min(1.0) * (-beat / 0.1).exp();
                let x = (TAU * 50.0 * beat).sin() * env * thump + (TAU * 7_000.0 * t).sin() * 0.08;
                [x, x]
            })
            .collect();
        let out = listen::mono(&through(&mut Master::new(SR), &input, drive));
        let power = listen::spectrum(&out, 8192);
        let around = listen::band_power(&power, SR, 6_000.0, 8_000.0);
        let hat = listen::band_power(&power, SR, 6_975.0, 7_025.0);
        10.0 * ((around - hat) / hat).max(1e-12).log10()
    }

    #[test]
    fn a_hat_stays_clean_while_the_kick_hits() {
        // The test for "every sound distorts every other": at the default DRIVE a kick at a
        // sensible level leaves no mark on the hat …
        let warm = hat_dirt(0.6, 0.3);
        assert!(warm < -45.0, "warm: {warm:.1} dB of the hat is dirt");
        // … and even a mix that comes in far too hot is turned down rather than crushed
        let hot_mix = hat_dirt(1.6, 0.3);
        assert!(hot_mix < -17.0, "a hot mix: {hot_mix:.1} dB");
        // Fuzz is asked for, and heard
        let fuzz = hat_dirt(0.6, 1.0);
        assert!(fuzz > warm + 15.0, "fuzz: {fuzz:.1} dB");
    }

    #[test]
    fn the_limiter_is_down_before_the_peak_and_lets_go_slowly() {
        let mut lim = Limiter::new(1.0, SR);
        let lag = lim.latency();
        // Silence, one loud burst, silence
        let mut out = Vec::new();
        for i in 0..SR as usize {
            let x = if (10_000..10_400).contains(&i) { 4.0 } else { 0.1 };
            out.push(lim.run(x, x).0);
        }
        assert!(listen::peak(&out) <= 1.0 + 1e-5);
        // The quiet part right before the burst is already turned down …
        assert!(out[10_000 + lag - 2] < 0.05);
        // … and long after it the level is back
        assert!((out[10_400 + lag + (0.6 * SR) as usize] - 0.1).abs() < 0.005);
        assert!(lim.gain() > 0.95);
    }

    #[test]
    fn drive_adds_colour_not_volume() {
        // A bass note at half scale: clean at 0, overtones at "warm", fuzz at the top – and
        // about as loud all the way
        let tone = listen::sine(80.0, 0.5, 1.0, SR);
        let heard = |amount: f32| {
            let out = listen::mono(&through(&mut Master::new(SR), &tone, amount));
            let out = &out[8192..];
            (listen::db(listen::rms(out)), listen::dirt_db(out, SR, &[80.0], 25.0))
        };
        let (clean, clean_dirt) = heard(0.0);
        let (warm, warm_dirt) = heard(0.3);
        let (fuzz, fuzz_dirt) = heard(1.0);
        assert!(clean_dirt < -50.0, "clean is clean: {clean_dirt:.1} dB");
        assert!(
            warm_dirt > clean_dirt + 10.0 && warm_dirt < -22.0,
            "warm is warm: {warm_dirt:.1} dB"
        );
        assert!(fuzz_dirt > -15.0, "fuzz is fuzz: {fuzz_dirt:.1} dB");
        assert!(
            (clean - warm).abs() < 1.0 && (clean - fuzz).abs() < 3.5,
            "{clean:.1} {warm:.1} {fuzz:.1} dB"
        );
    }

    #[test]
    fn the_soft_clipper_is_smooth() {
        // No step and no kink: neighbours stay neighbours all the way up
        let mut last = soft_clip(0.0);
        for i in 1..4000 {
            let y = soft_clip(i as f32 / 2000.0);
            assert!(y >= last && y - last <= 0.000_51, "{i}: {last} → {y}");
            last = y;
        }
        assert_eq!(soft_clip(5.0), CEILING);
        assert_eq!(soft_clip(-5.0), -CEILING);
    }

    #[test]
    fn a_constant_offset_is_removed() {
        let mut m = Master::new(SR);
        let offset: Vec<f32> = vec![0.3; SR as usize * 2];
        let out = through(&mut m, &offset, 0.0);
        assert!(out[out.len() - 2].abs() < 1e-3);
    }
}
