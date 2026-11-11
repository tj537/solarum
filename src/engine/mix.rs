//! The master section's per-sample effects, called from `Engine::render` after the voices
//! and sends: the master filters, crush, gate and tape stop.

use super::*;

/// Crush: the sample rate it holds at, and its steps (on the square root of the level, about 6 bits)
const CRUSH_RATE: f32 = 9000.0;
const CRUSH_STEPS: f32 = 24.0;
/// Tape stop: seconds to stand still, and to spin back up when let go
const TAPE_STOP: f32 = 0.8;
const TAPE_START: f32 = 0.35;

impl Engine {
    /// The master filters: the filter pad (with no hand on it, a breakdown's sweep), the
    /// build-up's high-pass, the DARK tone and the break zone's low-pass
    pub(super) fn apply_master_filters(&mut self, mut l: f32, mut r: f32, energy: f32, es_coef: f32) -> (f32, f32) {
        let sr = self.sr;
        // With no hand on the filter, a breakdown may sweep it open by itself
        let auto_filter = match &self.breakdown {
            Some(b) if b.sweep && self.playing => {
                let p = ((self.phase - b.start as f64) / b.len as f64).clamp(0.0, 1.0) as f32;
                -0.75 * (1.0 - p)
            }
            _ => 0.0,
        };
        let target = if self.filter_target.abs() > 0.01 {
            self.filter_target
        } else {
            auto_filter
        };
        self.filter += (target - self.filter) * 0.002;
        self.res += (self.res_target - self.res) * 0.002;
        // Q 1.6 at rest (as it always was), up to about 16 – a singing, whistling filter –
        // with the resonance all the way up
        let q = 1.6 + 14.0 * self.res.powf(1.3);
        let f = self.filter;
        if f < -0.02 {
            let fc = 18_000.0 * 0.012f32.powf(-f);
            self.filter_l.set(fc, q, sr);
            self.filter_r.tune_like(&self.filter_l);
            l = self.filter_l.run(l).lp;
            r = self.filter_r.run(r).lp;
        } else if f > 0.02 {
            let fc = 20.0 * 120f32.powf(f);
            self.filter_l.set(fc, q, sr);
            self.filter_r.tune_like(&self.filter_l);
            l = self.filter_l.run(l).hp;
            r = self.filter_r.run(r).hp;
        }
        if self.build_cut > 0.002 {
            // A light high-pass opens upwards under the gate
            let fc = 20.0 * 80f32.powf(self.build_cut * 0.6);
            self.build_hp_l.set(fc, 0.9, sr);
            self.build_hp_r.tune_like(&self.build_hp_l);
            l = self.build_hp_l.run(l).hp;
            r = self.build_hp_r.run(r).hp;
        }
        l = self.tone_l.run(l).lp;
        r = self.tone_r.run(r).lp;
        self.energy_smooth += (energy - self.energy_smooth) * es_coef;
        let brk = ((BREAK_ZONE - self.energy_smooth) / BREAK_ZONE).clamp(0.0, 1.0);
        if brk > 0.001 {
            let fc = 18_000.0 * 0.12f32.powf(brk);
            self.break_lp_l.set(fc, 0.8, sr);
            self.break_lp_r.tune_like(&self.break_lp_l);
            l = self.break_lp_l.run(l).lp;
            r = self.break_lp_r.run(r).lp;
        }
        (l, r)
    }

    /// Crush: one fixed lo-fi sound (a low sample rate, few bits) faded in and out, so switching
    /// it never steps through other settings. The bits are companded: quiet tails get grainy
    /// instead of dropping out.
    pub(super) fn apply_crush(&mut self, mut l: f32, mut r: f32, on: bool, coef: f32, lp: f32) -> (f32, f32) {
        self.crush += ((if on { 1.0 } else { 0.0 }) - self.crush) * coef;
        if self.crush > 1e-3 {
            self.crush_count += CRUSH_RATE / self.sr;
            if self.crush_count >= 1.0 {
                self.crush_count -= 1.0;
                let q = |x: f32| {
                    let y = (x.abs().sqrt() * CRUSH_STEPS).round() / CRUSH_STEPS;
                    (y * y).copysign(x)
                };
                self.crush_hold = (q(l), q(r));
            }
            self.crush_lp.0 += (self.crush_hold.0 - self.crush_lp.0) * lp;
            self.crush_lp.1 += (self.crush_hold.1 - self.crush_lp.1) * lp;
            let c = self.crush;
            let m = c * c * (3.0 - 2.0 * c);
            l += (self.crush_lp.0 - l) * m;
            r += (self.crush_lp.1 - r) * m;
        } else {
            self.crush_lp = (l, r);
        }
        (l, r)
    }

    /// Gate: open for the first half of every 16th, `want` 0 (off) … 1 (full)
    pub(super) fn apply_gate(&mut self, mut l: f32, mut r: f32, want: f32, coef: f32) -> (f32, f32) {
        self.gate += (want - self.gate) * 0.002;
        if self.gate > 0.002 {
            let open = !self.playing || self.phase.fract() < 0.5;
            self.gate_env += ((if open { 1.0 } else { 0.0 }) - self.gate_env) * coef;
            let g = 1.0 - self.gate * (1.0 - self.gate_env);
            l *= g;
            r *= g;
        }
        (l, r)
    }

    /// Tape stop: read the recent output back slower and slower until it stands still. Let go
    /// and it spins back up, landing on the live beat, then hands over to it.
    pub(super) fn apply_tape_stop(&mut self, mut l: f32, mut r: f32, on: bool) -> (f32, f32) {
        let sr = self.sr;
        let n = self.tape.len() / 2;
        let w = self.tape_w;
        self.tape[2 * w] = l;
        self.tape[2 * w + 1] = r;
        self.tape_w = (w + 1) % n;
        if on {
            if !self.taping {
                self.taping = true;
                // Pressed again while it spins up: slow down from where it is
                if self.tape_mix <= 0.0 {
                    self.tape_r = w as f64;
                    self.tape_rate = 1.0;
                }
            }
            self.tape_mix = 1.0;
            self.tape_rate = (self.tape_rate - 1.0 / (TAPE_STOP * sr)).max(0.0);
        } else if self.tape_mix > 0.0 {
            if self.taping {
                self.taping = false;
                // Stood still (and silent): start the spin-up just far enough back that it
                // catches up with the live output when it's back at full speed
                if self.tape_rate == 0.0 {
                    let lag = (TAPE_START * sr * 0.5) as usize;
                    self.tape_r = ((w + n - lag) % n) as f64;
                }
            }
            if self.tape_rate < 1.0 {
                self.tape_rate = (self.tape_rate + 1.0 / (TAPE_START * sr)).min(1.0);
            } else {
                self.tape_mix = (self.tape_mix - 1.0 / (0.02 * sr)).max(0.0);
            }
        }
        if self.tape_mix > 0.0 {
            let i = self.tape_r.floor() as usize % n;
            let j = (i + 1) % n;
            let f = self.tape_r.fract() as f32;
            let fade = (self.tape_rate * 3.0).min(1.0);
            let tl = (self.tape[2 * i] * (1.0 - f) + self.tape[2 * j] * f) * fade;
            let tr = (self.tape[2 * i + 1] * (1.0 - f) + self.tape[2 * j + 1] * f) * fade;
            l += (tl - l) * self.tape_mix;
            r += (tr - r) * self.tape_mix;
            self.tape_r = (self.tape_r + self.tape_rate as f64) % n as f64;
        }
        (l, r)
    }
}
