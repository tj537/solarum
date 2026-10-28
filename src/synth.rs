//! All sounds of Solarum, synthesized sample by sample – no recordings.
//!
//! Drums follow the classic analog recipes (808/909): a sine with a falling
//! pitch for the kick, filtered noise bursts for the clap, six detuned square
//! waves for the metallic hats and ride. Bass and acid are a saw wave through a
//! resonant filter with an envelope, the acid one 303-style with accent and slide.

use crate::rng::Rng;
use std::f32::consts::{PI, TAU};

pub fn midi_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

/// Exponential decay factor per sample for a time constant in seconds
fn decay_coef(seconds: f32, sr: f32) -> f32 {
    (-1.0 / (seconds.max(1e-4) * sr)).exp()
}

/// Soft clipper
pub fn sat(x: f32) -> f32 {
    x.tanh()
}

/// State-variable filter (Simper/Cytomic TPT): stable at any cutoff and resonance
#[derive(Clone, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

pub struct SvfOut {
    pub lp: f32,
    pub bp: f32,
    pub hp: f32,
}

impl Svf {
    pub fn new(cutoff: f32, q: f32, sr: f32) -> Self {
        let mut f = Svf::default();
        f.set(cutoff, q, sr);
        f
    }

    pub fn set(&mut self, cutoff: f32, q: f32, sr: f32) {
        let fc = cutoff.clamp(10.0, sr * 0.45);
        let g = (PI * fc / sr).tan();
        self.k = 1.0 / q.max(0.05);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// Take another filter's settings (the right channel of a stereo pair: one `tan`, not two)
    pub fn tune_like(&mut self, o: &Svf) {
        (self.a1, self.a2, self.a3, self.k) = (o.a1, o.a2, o.a3, o.k);
    }

    pub fn run(&mut self, x: f32) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        // Guard against denormals / blow-ups from extreme settings
        if !self.ic1.is_finite() || !self.ic2.is_finite() {
            self.ic1 = 0.0;
            self.ic2 = 0.0;
        }
        SvfOut {
            lp: v2,
            bp: v1,
            hp: x - self.k * v1 - v2,
        }
    }
}

/// Band-limited saw (PolyBLEP) – less harsh aliasing in the high notes
#[derive(Clone, Default)]
pub struct Saw {
    phase: f32,
}

impl Saw {
    fn next(&mut self, hz: f32, sr: f32) -> f32 {
        let dt = (hz / sr).min(0.5);
        let mut y = 2.0 * self.phase - 1.0;
        let t = self.phase;
        if t < dt {
            let t = t / dt;
            y -= t + t - t * t - 1.0;
        } else if t > 1.0 - dt {
            let t = (t - 1.0) / dt;
            y -= t * t + t + t + 1.0;
        }
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y
    }
}

// ───────────────────────────── Drums ─────────────────────────────

/// How a kick sound is made: pitch sweep, click, drive and length
pub struct KickModel {
    /// Start pitch as a multiple of the tune above it
    pub sweep: f32,
    /// Seconds the pitch takes to fall
    pub sweep_t: f32,
    pub click: f32,
    /// Multiplies and adds to the drive (distortion)
    pub drive: f32,
    pub drive_add: f32,
    pub length: f32,
    pub tune: f32,
    pub gain: f32,
}

/// In `Voice::Kick.sounds()` order: 909, 808, Hard, Deep, Punch, Rumble, Distort
pub const KICKS: [KickModel; 7] = [
    KickModel {
        sweep: 2.8,
        sweep_t: 0.026,
        click: 0.35,
        drive: 1.0,
        drive_add: 0.0,
        length: 1.0,
        tune: 1.0,
        gain: 1.0,
    },
    KickModel {
        sweep: 1.3,
        sweep_t: 0.045,
        click: 0.06,
        drive: 0.45,
        drive_add: 0.0,
        length: 2.6,
        tune: 0.92,
        gain: 0.9,
    },
    KickModel {
        sweep: 5.0,
        sweep_t: 0.016,
        click: 0.6,
        drive: 1.3,
        drive_add: 5.0,
        length: 1.35,
        tune: 1.05,
        gain: 0.72,
    },
    KickModel {
        sweep: 1.9,
        sweep_t: 0.034,
        click: 0.08,
        drive: 0.55,
        drive_add: 0.0,
        length: 1.7,
        tune: 0.86,
        gain: 1.0,
    },
    KickModel {
        sweep: 3.6,
        sweep_t: 0.011,
        click: 0.55,
        drive: 1.1,
        drive_add: 0.6,
        length: 0.62,
        tune: 1.14,
        gain: 1.1,
    },
    // Rumble: low, long and a little dirty – made for the rumble delays
    KickModel {
        sweep: 1.6,
        sweep_t: 0.05,
        click: 0.05,
        drive: 0.9,
        drive_add: 1.5,
        length: 2.2,
        tune: 0.82,
        gain: 0.64,
    },
    // Distort: a hard sweep into heavy clipping
    KickModel {
        sweep: 4.2,
        sweep_t: 0.02,
        click: 0.5,
        drive: 1.6,
        drive_add: 9.0,
        length: 1.1,
        tune: 1.0,
        gain: 0.56,
    },
];

#[derive(Clone)]
pub struct Kick {
    phase: f32,
    t: f32,
    amp: f32,
    env: f32,
    decay: f32,
    click: f32,
    pub tune: f32,
    pub drive: f32,
    pub sweep: f32,
    pub sweep_t: f32,
    pub click_amount: f32,
    pub gain: f32,
}

impl Kick {
    pub fn new() -> Self {
        Kick {
            phase: 0.0,
            t: 10.0,
            amp: 0.0,
            env: 0.0,
            decay: 1.0,
            click: 0.0,
            tune: 48.0,
            drive: 1.6,
            sweep: 2.8,
            sweep_t: 0.026,
            click_amount: 0.35,
            gain: 1.0,
        }
    }

    /// Take over the character of a kick sound (the engine sets tune and drive)
    pub fn set_model(&mut self, m: &KickModel) {
        self.sweep = m.sweep;
        self.sweep_t = m.sweep_t;
        self.click_amount = m.click;
        self.gain = m.gain;
    }

    /// `length` in seconds (0.2 … 0.7)
    pub fn trigger(&mut self, vel: f32, length: f32, sr: f32) {
        self.t = 0.0;
        self.phase = 0.0;
        self.amp = vel;
        self.env = 1.0;
        self.decay = decay_coef(length, sr);
        self.click = 1.0;
    }

    /// Envelope of the last hit – used to duck the bass and the rumble
    pub fn envelope(&self) -> f32 {
        self.env
    }

    pub fn next(&mut self, sr: f32, rng: &mut Rng) -> f32 {
        if self.env < 1e-4 {
            return 0.0;
        }
        let t = self.t / sr;
        let hz = self.tune + self.tune * self.sweep * (-t / self.sweep_t).exp() + 40.0 * (-t / 0.004).exp();
        self.phase = (self.phase + hz / sr) % 1.0;
        let attack = (t / 0.0015).min(1.0);
        let body = (self.phase * TAU).sin() * self.env * attack;
        let click = rng.noise() * self.click * self.click_amount;
        self.click *= 0.985;
        self.env *= self.decay;
        self.t += 1.0;
        sat((body + click) * self.drive) / sat(self.drive) * self.amp * self.gain
    }
}

/// Clap, snare, big clap or snap – all noise through a band-pass, the snare with a tone body
#[derive(Clone)]
pub struct Clap {
    t: f32,
    amp: f32,
    bp: Svf,
    hp: Svf,
    tail: f32,
    tail_decay: f32,
    burst_decay: f32,
    model: u8,
    body: f32,
    pitch: f32,
    /// Lo-fi: the held sample and the samples since it was taken
    hold: f32,
    held: f32,
}

impl Clap {
    pub fn new(sr: f32) -> Self {
        Clap {
            t: 1e9,
            amp: 0.0,
            bp: Svf::new(1150.0, 1.3, sr),
            hp: Svf::new(1800.0, 0.7, sr),
            tail: 0.0,
            tail_decay: decay_coef(0.12, sr),
            burst_decay: 0.0,
            model: 0,
            body: 0.0,
            pitch: 1.0,
            hold: 0.0,
            held: 0.0,
        }
    }

    /// `model` in `Voice::Clap.sounds()` order: clap, snare, big clap, snap, noise, lo-fi; `pitch` as a factor
    pub fn trigger(&mut self, vel: f32, length: f32, sr: f32, model: u8, pitch: f32) {
        self.t = 0.0;
        self.amp = vel;
        self.tail = 1.0;
        self.model = model;
        self.body = 0.0;
        self.pitch = pitch;
        let (band, q, len) = match model {
            1 => (3200.0, 0.8, length * 0.9),
            2 => (950.0, 1.0, length * 2.2),
            // Snap: lower and wider than it used to be, so it clicks without piercing
            3 => (1650.0, 1.0, 0.045),
            // Noise: a long, bright industrial snare
            4 => (5200.0, 0.6, length * 1.7),
            _ => (1150.0, 1.3, length),
        };
        self.bp.set(band * pitch, q, sr);
        self.hp.set(1800.0 * pitch, 0.7, sr);
        self.tail_decay = decay_coef(len, sr);
        self.burst_decay = decay_coef(0.006, sr);
    }

    pub fn next(&mut self, sr: f32, rng: &mut Rng) -> f32 {
        let ms = self.t / sr * 1000.0;
        if ms > 1500.0 {
            return 0.0;
        }
        self.t += 1.0;
        let n = rng.noise();
        match self.model {
            1 => {
                // Snare: a short tone body plus bright noise
                self.tail *= self.tail_decay;
                let t = ms / 1000.0;
                self.body = (self.body + (175.0 + 60.0 * (-t / 0.01).exp()) * self.pitch / sr) % 1.0;
                let tone = ((self.body * TAU).sin() + (self.body * TAU * 1.87).sin() * 0.4) * (-t / 0.045).exp();
                let noise = self.hp.run(self.bp.run(n).bp + n * 0.3).hp * self.tail;
                (tone * 0.75 + noise * 1.6) * self.amp * (t / 0.0008).min(1.0)
            }
            3 => {
                self.tail *= self.tail_decay;
                // A soft half-millisecond attack takes the sting out of the click
                let attack = (ms / 0.5).min(1.0);
                self.bp.run(n).bp * self.tail * self.amp * attack * 3.7
            }
            4 => {
                self.tail *= self.tail_decay;
                let attack = (ms / 1.0).min(1.0);
                self.hp.run(self.bp.run(n).bp + n * 0.4).hp * self.tail * self.amp * attack * 1.0
            }
            m => {
                // Quick bursts (hands not clapping exactly together), then the tail
                let (bursts, gap) = if m == 2 { (4.0, 11.0) } else { (3.0, 9.0) };
                let env = if ms < bursts * gap {
                    let local = (ms % gap) / 1000.0 * sr;
                    self.burst_decay.powf(local)
                } else {
                    self.tail *= self.tail_decay;
                    self.tail * 0.8
                };
                let x = self.bp.run(n).bp * env * self.amp * if m == 2 { 2.4 } else { 2.6 };
                if m != 5 {
                    return x;
                }
                // Lo-fi: 7 kHz sample rate and a dozen levels
                self.held += 1.0;
                if self.held >= sr / 7000.0 {
                    self.held -= sr / 7000.0;
                    self.hold = (x * 12.0).round() / 12.0;
                }
                self.hold * 1.1
            }
        }
    }
}

/// Rim, cowbell, clave, tom or blip – the small percussion
#[derive(Clone)]
pub struct Rim {
    t: f32,
    amp: f32,
    hp: Svf,
    bp: Svf,
    pitch: f32,
    model: u8,
    phases: [f32; 2],
    decay: f32,
}

impl Rim {
    pub fn new(sr: f32) -> Self {
        Rim {
            t: 1e9,
            amp: 0.0,
            hp: Svf::new(350.0, 0.7, sr),
            bp: Svf::new(800.0, 2.0, sr),
            pitch: 1.0,
            model: 0,
            phases: [0.0; 2],
            decay: 1.0,
        }
    }

    /// `model` in `Voice::Rim.sounds()` order: rim, cowbell, clave, tom, blip, conga, zap; `decay` stretches the envelopes
    pub fn trigger(&mut self, vel: f32, pitch: f32, model: u8, decay: f32) {
        self.t = 0.0;
        self.amp = vel;
        self.pitch = pitch;
        self.model = model;
        self.phases = [0.0; 2];
        self.decay = decay.max(0.05);
    }

    pub fn next(&mut self, sr: f32, rng: &mut Rng) -> f32 {
        let t = self.t / sr;
        if t > 0.5 * self.decay.max(1.0) {
            return 0.0;
        }
        // Envelopes run on stretched time, oscillators on real time
        let te = t / self.decay;
        self.t += 1.0;
        let p = self.pitch;
        match self.model {
            1 => {
                // 808 cowbell: two square waves through a band-pass
                for (ph, f) in self.phases.iter_mut().zip([540.0, 800.0]) {
                    *ph = (*ph + f * p / sr) % 1.0;
                }
                let sq = |ph: f32| if ph < 0.5 { 1.0 } else { -1.0 };
                let x = sq(self.phases[0]) + sq(self.phases[1]);
                let env = 0.6 * (-te / 0.012).exp() + 0.4 * (-te / 0.11).exp();
                self.bp.run(x).bp * env * self.amp * 0.32
            }
            2 => (TAU * 2450.0 * p * t).sin() * (-te / 0.02).exp() * self.amp * 0.8,
            3 => {
                let hz = (105.0 + 150.0 * (-te / 0.035).exp()) * p;
                self.phases[0] = (self.phases[0] + hz / sr) % 1.0;
                sat((self.phases[0] * TAU).sin() * 1.3) * (-te / 0.17).exp() * self.amp * 0.4
            }
            4 => {
                // FM blip
                let index = 3.0 * (-te / 0.012).exp();
                let m = (TAU * 1730.0 * p * t).sin() * index;
                (TAU * 1150.0 * p * t + m).sin() * (-te / 0.045).exp() * self.amp * 0.7
            }
            5 => {
                // Conga: a sine that drops a little into its note, with a slap
                let hz = (330.0 + 90.0 * (-te / 0.008).exp()) * p;
                self.phases[0] = (self.phases[0] + hz / sr) % 1.0;
                let slap = rng.noise() * (-te / 0.002).exp() * 0.3;
                ((self.phases[0] * TAU).sin() * (-te / 0.12).exp() + slap) * self.amp * 0.42
            }
            6 => {
                // Zap: a fast laser sweep down
                let hz = (180.0 + 2800.0 * (-te / 0.012).exp()) * p;
                self.phases[0] = (self.phases[0] + hz / sr) % 1.0;
                (self.phases[0] * TAU).sin() * (-te / 0.08).exp() * self.amp * 0.55
            }
            _ => {
                let tone = (TAU * 1720.0 * p * t).sin() * 0.55 + (TAU * 495.0 * p * t).sin() * 0.7;
                let x = tone * (-te / 0.024).exp() + rng.noise() * (-te / 0.003).exp() * 0.6;
                self.hp.run(x).hp * self.amp
            }
        }
    }
}

/// Six square waves at inharmonic ratios – the 808's metallic source
#[derive(Clone)]
struct Metal {
    phases: [f32; 6],
    freqs: [f32; 6],
}

impl Metal {
    fn new(scale: f32) -> Self {
        let mut m = Metal {
            phases: [0.0, 0.13, 0.31, 0.57, 0.71, 0.89],
            freqs: [0.0; 6],
        };
        m.tune(scale);
        m
    }

    fn tune(&mut self, scale: f32) {
        self.freqs = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0].map(|f| f * scale);
    }

    fn next(&mut self, sr: f32) -> f32 {
        let mut s = 0.0;
        for (p, f) in self.phases.iter_mut().zip(self.freqs) {
            *p = (*p + f / sr) % 1.0;
            s += if *p < 0.5 { 1.0 } else { -1.0 };
        }
        s / 6.0
    }
}

/// How a cymbal sound is made
pub struct HatModel {
    /// Pitch of the metal oscillators
    pub scale: f32,
    pub band: f32,
    pub hp: f32,
    /// 0 = pure metal, 1 = pure noise
    pub noise: f32,
    /// Multiplies the length the engine asks for
    pub length: f32,
    /// Fade-in time in seconds (shakers swell)
    pub rise: f32,
    pub gain: f32,
}

const fn hat(scale: f32, band: f32, hp: f32, noise: f32, length: f32, rise: f32, gain: f32) -> HatModel {
    HatModel {
        scale,
        band,
        hp,
        noise,
        length,
        rise,
        gain,
    }
}

/// Closed hats: 909, 808, shaker, tick, metal, crisp
pub const HATS: [HatModel; 6] = [
    hat(1.0, 10_000.0, 7_000.0, 0.45, 1.0, 0.0, 1.0),
    hat(1.0, 10_000.0, 7_000.0, 0.05, 0.9, 0.0, 2.6),
    hat(1.0, 7_000.0, 4_500.0, 1.0, 1.5, 0.012, 0.5),
    hat(1.6, 12_000.0, 9_000.0, 0.3, 0.4, 0.0, 2.2),
    hat(1.35, 9_500.0, 6_000.0, 0.0, 0.7, 0.0, 2.8),
    hat(1.0, 13_000.0, 10_000.0, 1.0, 0.6, 0.0, 0.7),
];
/// Open hats: 909, 808, sizzle, long, metal
pub const OPEN_HATS: [HatModel; 5] = [
    hat(1.0, 9_000.0, 6_500.0, 0.4, 1.0, 0.0, 1.0),
    hat(1.0, 9_000.0, 6_500.0, 0.05, 1.0, 0.0, 2.3),
    hat(1.25, 11_000.0, 8_000.0, 0.7, 1.6, 0.004, 0.55),
    hat(1.0, 9_000.0, 6_500.0, 0.4, 2.2, 0.0, 0.95),
    hat(1.45, 8_500.0, 5_500.0, 0.05, 1.3, 0.0, 2.1),
];
/// Rides: ride, bell, dark, crash, shimmer
pub const RIDES: [HatModel; 5] = [
    hat(1.37, 6_200.0, 3_800.0, 0.15, 1.0, 0.0, 1.2),
    hat(2.1, 4_200.0, 2_600.0, 0.02, 0.55, 0.0, 1.0),
    hat(0.9, 4_600.0, 2_800.0, 0.3, 1.2, 0.0, 1.0),
    hat(1.2, 7_500.0, 4_500.0, 0.65, 2.4, 0.0, 0.42),
    hat(2.6, 7_000.0, 4_800.0, 0.1, 1.4, 0.0, 1.0),
];

#[derive(Clone)]
pub struct Hat {
    metal: Metal,
    bp: Svf,
    hp: Svf,
    env: f32,
    decay: f32,
    amp: f32,
    noise_mix: f32,
    rise: f32,
    rise_coef: f32,
    gain: f32,
}

impl Hat {
    pub fn new(sr: f32, model: &HatModel) -> Self {
        let mut h = Hat {
            metal: Metal::new(model.scale),
            bp: Svf::new(model.band, 1.1, sr),
            hp: Svf::new(model.hp, 0.7, sr),
            env: 0.0,
            decay: 0.0,
            amp: 0.0,
            noise_mix: model.noise,
            rise: 1.0,
            rise_coef: 0.0,
            gain: 1.0,
        };
        h.set_model(model, 1.0, sr);
        h
    }

    /// `tune` as a factor (1 = the model's own pitch)
    pub fn set_model(&mut self, m: &HatModel, tune: f32, sr: f32) {
        self.metal.tune(m.scale * tune);
        self.bp.set(m.band * tune, 1.1, sr);
        self.hp.set(m.hp * tune, 0.7, sr);
        self.noise_mix = m.noise;
        self.rise_coef = if m.rise > 0.0 { decay_coef(m.rise, sr) } else { 0.0 };
        self.gain = m.gain;
    }

    pub fn trigger(&mut self, vel: f32, length: f32, sr: f32) {
        self.env = 1.0;
        self.amp = vel;
        self.decay = decay_coef(length, sr);
        self.rise = if self.rise_coef > 0.0 { 0.0 } else { 1.0 };
    }

    /// An open hat is cut off by the closed one
    pub fn choke(&mut self, sr: f32) {
        self.decay = self.decay.min(decay_coef(0.01, sr));
    }

    pub fn next(&mut self, sr: f32, rng: &mut Rng) -> f32 {
        if self.env < 1e-4 {
            return 0.0;
        }
        let src = self.metal.next(sr) * (1.0 - self.noise_mix) + rng.noise() * self.noise_mix;
        let x = self.bp.run(src).bp;
        let y = self.hp.run(x).hp;
        self.env *= self.decay;
        self.rise = 1.0 - (1.0 - self.rise) * self.rise_coef;
        y * self.env * self.rise * self.amp * self.gain * 2.2
    }
}

// ───────────────────────────── Synths ─────────────────────────────

/// The bass: rolling filtered saw, sub, reese, pulse or donk
#[derive(Clone)]
pub struct Bass {
    saw: Saw,
    saw2: Saw,
    sub: f32,
    lp: Svf,
    hz: f32,
    amp: f32,
    env: f32,
    fenv: f32,
    gate: f32,
    adecay: f32,
    fdecay: f32,
    model: u8,
}

impl Bass {
    pub fn new(sr: f32) -> Self {
        Bass {
            saw: Saw::default(),
            saw2: Saw { phase: 0.5 },
            sub: 0.0,
            lp: Svf::new(300.0, 0.9, sr),
            hz: 55.0,
            amp: 0.0,
            env: 0.0,
            fenv: 0.0,
            gate: 0.0,
            adecay: 0.0,
            fdecay: 0.0,
            model: 0,
        }
    }

    /// `gate` in samples; `model` in `Voice::Bass.sounds()` order; `decay` stretches the envelopes
    #[allow(clippy::too_many_arguments)]
    pub fn trigger(&mut self, note: f32, vel: f32, gate: f32, sr: f32, model: u8, decay: f32) {
        self.hz = midi_hz(note);
        self.amp = vel;
        self.env = 1.0;
        self.fenv = 1.0;
        self.gate = gate;
        if model != self.model {
            self.model = model;
            self.saw2.phase = (self.saw.phase + 0.5) % 1.0;
        }
        let (a, f) = match model {
            1 => (0.4, 0.05),
            2 => (0.6, 0.12),
            3 => (0.2, 0.06),
            4 => (0.13, 0.022),
            5 => (0.3, 0.08),
            6 => (0.35, 0.1),
            _ => (0.22, 0.055),
        };
        self.adecay = decay_coef(a * decay, sr);
        self.fdecay = decay_coef(f * decay.sqrt(), sr);
    }

    /// `bright` 0…1 (energy), `dark` 0…1 lowers the filter
    pub fn next(&mut self, sr: f32, bright: f32, dark: f32) -> f32 {
        if self.env < 1e-4 {
            return 0.0;
        }
        if self.gate > 0.0 {
            self.gate -= 1.0;
            self.env *= self.adecay;
        } else {
            self.env *= 0.995; // quick release
        }
        self.fenv *= self.fdecay;
        self.sub = (self.sub + self.hz / sr) % 1.0;
        let sub = (self.sub * TAU).sin();
        let open = 1.0 - dark;
        let y = match self.model {
            1 => {
                // Sub: a sine with a touch of drive
                return sat((sub + (self.sub * TAU * 2.0).sin() * 0.12 * (0.5 + bright)) * 1.3) * self.env * self.amp * 0.6;
            }
            2 => {
                // Reese: two detuned saws, slowly beating
                let x = (self.saw.next(self.hz * 1.007, sr) + self.saw2.next(self.hz * 0.993, sr)) * 0.45 + sub * 0.5;
                self.lp.set(200.0 + open * 320.0 + self.fenv * (250.0 + bright * 800.0), 1.1, sr);
                self.lp.run(x).lp * 0.95
            }
            3 => {
                // Pulse: a square from two saws half a cycle apart
                let x = (self.saw.next(self.hz, sr) - self.saw2.next(self.hz, sr)) * 0.45 + sub * 0.45;
                self.lp.set(110.0 + open * 200.0 + self.fenv * (450.0 + bright * 1300.0), 2.0, sr);
                self.lp.run(x).lp * 1.7
            }
            4 => {
                // Donk: a very resonant, very short pluck
                let x = self.saw.next(self.hz, sr) * 0.8 + sub * 0.5;
                self.lp.set(140.0 + open * 120.0 + self.fenv * (2200.0 + bright * 1600.0), 7.0, sr);
                self.lp.run(x).lp * 1.1
            }
            5 => {
                // FM: a sine bending itself, the index falls with the envelope
                let index = 0.6 + self.fenv * (2.5 + bright * 2.0);
                (self.sub * TAU + index * sub).sin() * 0.62
            }
            6 => {
                // Growl: a driven saw into a resonant filter
                let x = sat((self.saw.next(self.hz, sr) * 0.9 + sub * 0.4) * 3.0);
                self.lp.set(180.0 + open * 300.0 + self.fenv * (900.0 + bright * 1500.0), 3.0, sr);
                self.lp.run(x).lp * 0.9
            }
            _ => {
                let x = self.saw.next(self.hz, sr) * 0.8 + sub * 0.5;
                self.lp.set(90.0 + open * 160.0 + self.fenv * (350.0 + bright * 1100.0), 1.4, sr);
                self.lp.run(x).lp * 1.8
            }
        };
        sat(y) * self.env * self.amp
    }
}

/// The synth line: 303 acid (saw or square), chord stab, pluck, rave hoover, pad, organ or supersaw.
/// Up to six saw oscillators into a resonant low-pass with envelope, accent and slide.
#[derive(Clone)]
pub struct Acid {
    saws: [Saw; 6],
    /// Frequency ratio and gain per oscillator
    ratio: [f32; 6],
    gain: [f32; 6],
    lp: Svf,
    lp2: Svf,
    hz: f32,
    target_hz: f32,
    glide: f32,
    amp: f32,
    env: f32,
    adecay: f32,
    fenv: f32,
    fdecay: f32,
    gate: f32,
    accent: f32,
    /// Hoover: pitch rises into the note
    bend: f32,
    bend_coef: f32,
    /// Pad: the swell into the chord (0 → 1)
    rise: f32,
    rise_coef: f32,
    model: u8,
}

impl Acid {
    pub fn new(sr: f32) -> Self {
        let mut a = Acid {
            saws: Default::default(),
            ratio: [1.0; 6],
            gain: [0.0; 6],
            lp: Svf::new(800.0, 4.0, sr),
            lp2: Svf::new(800.0, 0.7, sr),
            hz: 110.0,
            target_hz: 110.0,
            glide: 0.0,
            amp: 0.0,
            env: 0.0,
            adecay: 1.0,
            fenv: 0.0,
            fdecay: 0.0,
            gate: 0.0,
            accent: 0.0,
            bend: 0.0,
            bend_coef: 0.0,
            rise: 1.0,
            rise_coef: 0.0,
            model: u8::MAX,
        };
        a.set_model(0, [0.0, 3.0, 7.0]);
        a
    }

    /// Oscillator layout for a sound; `chord` = semitones of root, third and fifth (stab)
    fn set_model(&mut self, model: u8, chord: [f32; 3]) {
        let st = |s: f32| 2f32.powf(s / 12.0);
        self.ratio = [1.0; 6];
        self.gain = [0.0; 6];
        match model {
            1 => {
                // Square: the second saw half a cycle later, subtracted
                self.gain[0] = 1.0;
                self.gain[1] = -1.0;
            }
            2 | 5 => {
                for (i, &c) in chord.iter().enumerate() {
                    self.ratio[i * 2] = st(c + 0.08);
                    self.ratio[i * 2 + 1] = st(c - 0.08);
                    self.gain[i * 2] = 0.42;
                    self.gain[i * 2 + 1] = 0.42;
                }
            }
            3 => {
                self.gain[0] = 1.0;
                self.ratio[1] = 2.0;
                self.gain[1] = 0.45;
            }
            4 => {
                for (i, d) in [-0.32f32, -0.12, 0.0, 0.12, 0.32, -12.0].into_iter().enumerate() {
                    self.ratio[i] = st(d);
                    self.gain[i] = if i == 5 { 0.5 } else { 0.38 };
                }
            }
            6 => {
                // Organ: the chord plus its octave
                for (i, &c) in chord.iter().enumerate() {
                    self.ratio[i * 2] = st(c);
                    self.ratio[i * 2 + 1] = st(c + 12.0);
                    self.gain[i * 2] = 0.4;
                    self.gain[i * 2 + 1] = 0.25;
                }
            }
            7 => {
                for (i, d) in [-0.25f32, -0.1, -0.03, 0.03, 0.1, 0.25].into_iter().enumerate() {
                    self.ratio[i] = st(d);
                    self.gain[i] = 0.3;
                }
            }
            _ => self.gain[0] = 1.0,
        }
        if model != self.model {
            // Spread the phases so stacked saws don't start as one
            for (i, s) in self.saws.iter_mut().enumerate() {
                s.phase = if model == 1 && i == 1 { 0.5 } else { (i as f32 * 0.37) % 1.0 };
            }
            if model == 1 {
                self.saws[1].phase = (self.saws[0].phase + 0.5) % 1.0;
            }
        }
        self.model = model;
    }

    /// `model` in `Voice::Acid.sounds()` order; `chord` only matters for the stab
    #[allow(clippy::too_many_arguments)]
    pub fn trigger(&mut self, note: f32, vel: f32, accent: bool, gate: f32, sr: f32, model: u8, chord: [f32; 3], decay: f32) {
        self.set_model(model, chord);
        self.hz = midi_hz(note);
        self.target_hz = self.hz;
        self.amp = vel;
        self.env = 1.0;
        self.fenv = 1.0;
        self.accent = if accent { 1.0 } else { 0.0 };
        let (fd, ad) = match model {
            2 => (0.14, 0.32),
            3 => (0.07, 0.2),
            4 => (0.3, 2.0),
            5 => (0.6, 3.0),
            6 => (0.05, 0.45),
            7 => (0.12, 0.4),
            _ => (if accent { 0.09 } else { 0.2 }, 1e3),
        };
        self.fdecay = decay_coef(fd * decay, sr);
        self.adecay = decay_coef(ad * decay, sr);
        self.bend = if model == 4 { 1.0 } else { 0.0 };
        self.bend_coef = decay_coef(0.07, sr);
        self.rise = if model == 5 { 0.0 } else { 1.0 };
        self.rise_coef = decay_coef(0.12 * decay, sr);
        self.gate = gate;
    }

    /// Slide: change pitch smoothly, keep the envelopes running
    pub fn slide(&mut self, note: f32, accent: bool, gate: f32, sr: f32) {
        self.target_hz = midi_hz(note);
        self.glide = decay_coef(0.045, sr);
        self.gate = gate;
        if accent {
            self.accent = 1.0;
            self.fenv = self.fenv.max(0.7);
        }
        if self.env < 0.05 {
            let model = self.model;
            self.trigger(note, self.amp.max(0.7), accent, gate, sr, model, [0.0, 3.0, 7.0], 1.0);
        }
    }

    /// `acid` 0…1: filter opening and resonance
    pub fn next(&mut self, sr: f32, acid: f32) -> f32 {
        if self.env < 1e-4 {
            return 0.0;
        }
        self.hz = self.target_hz + (self.hz - self.target_hz) * self.glide;
        if self.gate > 0.0 {
            self.gate -= 1.0;
            self.env *= self.adecay;
        } else {
            self.env *= match self.model {
                4 => 0.9995,
                5 => 0.99985,
                _ => 0.993,
            };
        }
        self.fenv *= self.fdecay;
        self.bend *= self.bend_coef;
        self.rise = 1.0 - (1.0 - self.rise) * self.rise_coef;
        let a = acid.clamp(0.0, 1.0);
        let (cutoff, q, drive, level) = match self.model {
            2 => (300.0 + a * 1800.0 + (1200.0 + a * 2600.0) * self.fenv, 1.2 + a * 3.0, 1.3, 1.0),
            3 => (350.0 + a * 1400.0 + (2200.0 + a * 3500.0) * self.fenv, 1.0 + a * 4.0, 1.4, 1.1),
            4 => (900.0 + a * 3500.0 + 1500.0 * self.fenv, 0.8 + a * 1.5, 1.6, 0.85),
            5 => (500.0 + a * 1500.0 + 800.0 * self.fenv, 0.8, 1.1, 1.1 * self.rise),
            6 => (900.0 + a * 2500.0 + 2500.0 * self.fenv, 0.7, 1.5, 1.2),
            7 => (1200.0 + a * 3500.0 + 3000.0 * self.fenv, 1.0, 1.3, 1.25),
            _ => {
                let base = 140.0 + a * a * 1800.0;
                let depth = (500.0 + a * 3600.0) * (1.0 + self.accent * 0.8);
                (
                    base + depth * self.fenv,
                    2.5 + a * 9.0 + self.accent * 2.0,
                    1.6 + a * 1.6,
                    if self.model == 1 { 0.85 } else { 1.0 },
                )
            }
        };
        self.lp.set(cutoff, q, sr);
        self.lp2.set(cutoff * 1.6, 0.6, sr);
        let hz = self.hz * 2f32.powf(-self.bend * 5.0 / 12.0);
        let mut x = 0.0;
        for i in 0..6 {
            if self.gain[i] != 0.0 {
                x += self.saws[i].next(hz * self.ratio[i], sr) * self.gain[i];
            }
        }
        let y = self.lp2.run(self.lp.run(x).lp).lp;
        let level = self.amp * (0.75 + self.accent * 0.35) * level;
        sat(y * drive) * self.env * level * 0.8
    }
}

/// A long low "boom" for the drop
#[derive(Clone, Default)]
pub struct Boom {
    phase: f32,
    t: f32,
    on: bool,
}

impl Boom {
    pub fn trigger(&mut self) {
        self.t = 0.0;
        self.phase = 0.0;
        self.on = true;
    }

    pub fn next(&mut self, sr: f32) -> f32 {
        if !self.on {
            return 0.0;
        }
        let t = self.t / sr;
        if t > 3.0 {
            self.on = false;
            return 0.0;
        }
        self.t += 1.0;
        let hz = 34.0 + 60.0 * (-t / 0.08).exp();
        self.phase = (self.phase + hz / sr) % 1.0;
        (self.phase * TAU).sin() * (-t / 0.9).exp() * (t / 0.002).min(1.0) * 0.9
    }
}

// ───────────────────────────── Effects ─────────────────────────────

/// Stereo ping-pong delay with a filtered feedback path
pub struct Delay {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    pos: usize,
    tone_l: Svf,
    tone_r: Svf,
}

impl Delay {
    pub fn new(sr: f32) -> Self {
        let n = (sr * 2.0) as usize;
        Delay {
            buf_l: vec![0.0; n],
            buf_r: vec![0.0; n],
            pos: 0,
            tone_l: Svf::new(2800.0, 0.6, sr),
            tone_r: Svf::new(2800.0, 0.6, sr),
        }
    }

    /// `time` in samples; input mono-ish (l, r); returns the wet signal
    pub fn run(&mut self, l: f32, r: f32, time: f32, feedback: f32) -> (f32, f32) {
        let n = self.buf_l.len();
        let d = (time as usize).clamp(1, n - 1);
        let read = (self.pos + n - d) % n;
        let (dl, dr) = (self.buf_l[read], self.buf_r[read]);
        let fl = self.tone_l.run(dr).lp;
        let fr = self.tone_r.run(dl).lp;
        // Ping-pong: left feeds right and vice versa
        self.buf_l[self.pos] = l + fl * feedback;
        self.buf_r[self.pos] = r * 0.3 + fr * feedback;
        self.pos = (self.pos + 1) % n;
        (dl, dr)
    }
}

/// A compact Freeverb: parallel combs into series all-passes, per channel
pub struct Reverb {
    combs: Vec<(Vec<f32>, usize, f32)>,
    allpasses: Vec<(Vec<f32>, usize)>,
}

impl Reverb {
    pub fn new(sr: f32) -> Self {
        let scale = sr / 44_100.0;
        let mut combs = Vec::new();
        for (i, &len) in [1116usize, 1188, 1277, 1356, 1422, 1491, 1557, 1617].iter().enumerate() {
            // Right channel uses slightly longer lines for width
            for ch in 0..2 {
                let l = ((len + ch * 23 + i) as f32 * scale) as usize;
                combs.push((vec![0.0; l], 0, 0.0));
            }
        }
        let mut allpasses = Vec::new();
        for &len in &[556usize, 441, 341, 225] {
            for ch in 0..2 {
                allpasses.push((vec![0.0; ((len + ch * 23) as f32 * scale) as usize], 0));
            }
        }
        Reverb { combs, allpasses }
    }

    /// `size` 0…1, `damp` 0…1
    pub fn run(&mut self, input: f32, size: f32, damp: f32) -> (f32, f32) {
        let fb = 0.7 + size * 0.28;
        let mut out = [0.0f32; 2];
        for (i, (buf, pos, store)) in self.combs.iter_mut().enumerate() {
            let y = buf[*pos];
            *store = y * (1.0 - damp) + *store * damp;
            buf[*pos] = input + *store * fb;
            *pos = (*pos + 1) % buf.len();
            out[i % 2] += y;
        }
        for (i, (buf, pos)) in self.allpasses.iter_mut().enumerate() {
            let ch = i % 2;
            let b = buf[*pos];
            buf[*pos] = out[ch] + b * 0.5;
            out[ch] = b - out[ch];
            *pos = (*pos + 1) % buf.len();
        }
        (out[0] * 0.06, out[1] * 0.06)
    }
}

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

    const SR: f32 = 44_100.0;

    fn peak_and_finite(mut f: impl FnMut() -> f32, n: usize) -> f32 {
        let mut peak = 0.0f32;
        for _ in 0..n {
            let x = f();
            assert!(x.is_finite());
            peak = peak.max(x.abs());
        }
        peak
    }

    #[test]
    fn voices_make_sound_and_stay_bounded() {
        let mut rng = Rng::new(1);
        let mut kick = Kick::new();
        kick.trigger(1.0, 0.5, SR);
        let p = peak_and_finite(|| kick.next(SR, &mut rng), 20_000);
        assert!(p > 0.5 && p <= 1.05, "kick {p}");

        for (m, model) in KICKS.iter().enumerate() {
            let mut kick = Kick::new();
            kick.set_model(model);
            kick.drive = 2.0 * model.drive + model.drive_add;
            kick.trigger(1.0, 0.3 * model.length, SR);
            let p = peak_and_finite(|| kick.next(SR, &mut rng), 30_000);
            assert!(p > 0.5 && p <= 1.05 * model.gain.max(1.0), "kick {m}: {p}");
        }

        for m in 0..4 {
            let mut clap = Clap::new(SR);
            clap.trigger(1.0, 0.15, SR, m, 1.0);
            let p = peak_and_finite(|| clap.next(SR, &mut rng), 20_000);
            assert!(p > 0.1 && p < 3.0, "clap {m}: {p}");
        }

        for m in 0..5 {
            let mut rim = Rim::new(SR);
            rim.trigger(1.0, 1.0, m, 1.0);
            let p = peak_and_finite(|| rim.next(SR, &mut rng), 20_000);
            assert!(p > 0.1 && p < 3.0, "rim {m}: {p}");
        }

        for model in HATS.iter().chain(&OPEN_HATS).chain(&RIDES) {
            let mut hat = Hat::new(SR, model);
            hat.trigger(1.0, 0.2, SR);
            let p = peak_and_finite(|| hat.next(SR, &mut rng), 20_000);
            assert!(p > 0.05 && p < 3.0, "hat {p}");
        }

        for m in 0..5 {
            let mut bass = Bass::new(SR);
            bass.trigger(41.0, 1.0, 4000.0, SR, m, 1.0);
            let p = peak_and_finite(|| bass.next(SR, 0.5, 0.5), 20_000);
            assert!(p > 0.2 && p <= 1.0, "bass {m}: {p}");
        }

        for m in 0..5 {
            let mut acid = Acid::new(SR);
            acid.trigger(53.0, 1.0, true, 4000.0, SR, m, [0.0, 3.0, 7.0], 1.0);
            let p = peak_and_finite(|| acid.next(SR, 1.0), 20_000);
            assert!(p > 0.2 && p <= 1.2, "acid {m}: {p}");
        }
    }

    #[test]
    fn voices_fall_silent() {
        let mut rng = Rng::new(2);
        let mut kick = Kick::new();
        kick.trigger(1.0, 0.3, SR);
        for _ in 0..(SR as usize * 4) {
            kick.next(SR, &mut rng);
        }
        assert_eq!(kick.next(SR, &mut rng), 0.0);
    }

    #[test]
    fn svf_lowpass_passes_dc_and_blocks_highs() {
        let mut f = Svf::new(200.0, 0.7, SR);
        let mut y = 0.0;
        for _ in 0..10_000 {
            y = f.run(1.0).lp;
        }
        assert!((y - 1.0).abs() < 0.01);
        let mut f = Svf::new(200.0, 0.7, SR);
        let mut peak = 0.0f32;
        for i in 0..10_000 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 }; // Nyquist
            peak = peak.max(f.run(x).lp.abs());
        }
        assert!(peak < 0.01);
    }
}
