//! Tests that listen. Nobody can hear a test run, so these measure what an ear would judge:
//! loudness (K-weighted, like LUFS), how squashed the mix is (crest factor), where the energy
//! sits in the spectrum, how wide the stereo picture is, and how much a sound gets distorted.
//!
//! Test-only: the engine's own tests assert on these numbers, and
//! `cargo test --release print_mix -- --ignored --nocapture` prints them for every vibe.

use std::f32::consts::TAU;

/// What one piece of interleaved stereo audio measures like
#[derive(Clone, Copy, Debug)]
pub struct Heard {
    /// Highest sample, 0…1
    pub peak: f32,
    /// Plain loudness in dB (RMS over both channels)
    pub rms_db: f32,
    /// Loudness as the ear weighs it (K-weighted, ungated) – close to LUFS
    pub lufs: f32,
    /// Peak over RMS in dB: under 7 is squashed flat, 9–14 punches
    pub crest_db: f32,
    /// A constant offset from zero (should be none)
    pub dc: f32,
    /// −1 (out of phase) … 0 (unrelated) … 1 (mono)
    pub width: f32,
    /// Share of the energy per band in dB (see `BANDS`)
    pub bands: [f32; 6],
}

/// Sub, bass, low mids, mids, presence, air – the upper edge of each band in Hz
pub const BANDS: [(&str, f32); 6] = [
    ("sub", 60.0),
    ("bass", 200.0),
    ("lowmid", 500.0),
    ("mid", 2_000.0),
    ("pres", 6_000.0),
    ("air", 22_050.0),
];

pub fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

pub fn mono(stereo: &[f32]) -> Vec<f32> {
    stereo.as_chunks::<2>().0.iter().map(|f| (f[0] + f[1]) * 0.5).collect()
}

pub fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0, |m, x| m.max(x.abs()))
}

pub fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| (x * x) as f64).sum::<f64>() / buf.len().max(1) as f64).sqrt() as f32
}

pub fn hear(stereo: &[f32], sr: f32) -> Heard {
    let n = (stereo.len() / 2).max(1) as f64;
    let (mut sl, mut sr_, mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for f in stereo.as_chunks::<2>().0 {
        let (l, r) = (f[0] as f64, f[1] as f64);
        sl += l;
        sr_ += r;
        ll += l * l;
        rr += r * r;
        lr += l * r;
    }
    let peak = peak(stereo);
    let rms = rms(stereo);
    let power = spectrum(&mono(stereo), 4096);
    let total: f32 = power.iter().sum::<f32>().max(1e-20);
    let mut lo = 0.0;
    let bands = BANDS.map(|(_, hi)| {
        let share = band_power(&power, sr, lo, hi) / total;
        lo = hi;
        10.0 * share.max(1e-12).log10()
    });
    Heard {
        peak,
        rms_db: db(rms),
        lufs: loudness(stereo, sr),
        crest_db: db(peak) - db(rms),
        dc: ((sl / n).abs().max((sr_ / n).abs())) as f32,
        width: (lr / (ll * rr).sqrt().max(1e-20)) as f32,
        bands,
    }
}

/// In-place radix-2 FFT (`re.len()` a power of two)
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -TAU / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f32).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
            }
        }
        len <<= 1;
    }
}

/// Power per frequency bin (0 … Nyquist), averaged over windows of `size` samples
/// (Blackman-Harris: a tone leaks less than −90 dB into its neighbours, so quiet dirt shows)
pub fn spectrum(mono: &[f32], size: usize) -> Vec<f32> {
    let window: Vec<f32> = (0..size)
        .map(|i| {
            let x = TAU * i as f32 / size as f32;
            0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()
        })
        .collect();
    let mut power = vec![0.0f32; size / 2];
    let mut frames = 0;
    let mut at = 0;
    while at + size <= mono.len() {
        let mut re: Vec<f32> = mono[at..at + size].iter().zip(&window).map(|(x, w)| x * w).collect();
        let mut im = vec![0.0; size];
        fft(&mut re, &mut im);
        for (p, (r, i)) in power.iter_mut().zip(re.iter().zip(&im)) {
            *p += r * r + i * i;
        }
        frames += 1;
        at += size / 2;
    }
    for p in &mut power {
        *p /= frames.max(1) as f32;
    }
    power
}

/// The power between `lo` and `hi` Hz in a spectrum from `spectrum`
pub fn band_power(power: &[f32], sr: f32, lo: f32, hi: f32) -> f32 {
    let hz = sr * 0.5 / power.len() as f32;
    power
        .iter()
        .enumerate()
        .filter(|(i, _)| (lo..hi).contains(&(*i as f32 * hz)))
        .map(|(_, p)| p)
        .sum()
}

/// The frequency the spectrum balances on – higher is brighter
pub fn centroid(power: &[f32], sr: f32) -> f32 {
    let hz = sr * 0.5 / power.len() as f32;
    let total: f32 = power.iter().sum::<f32>().max(1e-20);
    power.iter().enumerate().map(|(i, p)| i as f32 * hz * p).sum::<f32>() / total
}

struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }

    /// The K-weighting's first stage: a high shelf, +4 dB above about 1.7 kHz
    fn k_shelf(sr: f32) -> Self {
        let a = 10f32.powf(3.999_843_8 / 40.0);
        let w = TAU * 1_681.974_5 / sr;
        let alpha = w.sin() / (2.0 * 0.707_175_25);
        let (c, s) = (w.cos(), 2.0 * a.sqrt() * alpha);
        let a0 = (a + 1.0) - (a - 1.0) * c + s;
        Biquad {
            b: [
                a * ((a + 1.0) + (a - 1.0) * c + s) / a0,
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c) / a0,
                a * ((a + 1.0) + (a - 1.0) * c - s) / a0,
            ],
            a: [2.0 * ((a - 1.0) - (a + 1.0) * c) / a0, ((a + 1.0) - (a - 1.0) * c - s) / a0],
            z: [0.0; 2],
        }
    }

    /// The K-weighting's second stage: a high-pass at 38 Hz
    fn k_highpass(sr: f32) -> Self {
        let w = TAU * 38.135_47 / sr;
        let alpha = w.sin() / (2.0 * 0.500_327_05);
        let c = w.cos();
        let a0 = 1.0 + alpha;
        Biquad {
            b: [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0],
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }
}

/// Loudness after ITU-R BS.1770 without the gating – for music that plays all the way
/// through it lands within a dB of the LUFS a meter shows
pub fn loudness(stereo: &[f32], sr: f32) -> f32 {
    let mut sum = 0.0f64;
    for ch in 0..2 {
        let (mut shelf, mut hp) = (Biquad::k_shelf(sr), Biquad::k_highpass(sr));
        sum += stereo
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| {
                let y = hp.run(shelf.run(f[ch]));
                (y * y) as f64
            })
            .sum::<f64>();
    }
    let n = (stereo.len() / 2).max(1) as f64;
    -0.691 + 10.0 * (sum / n).max(1e-12).log10() as f32
}

/// A sine of `hz` at amplitude `amp`, interleaved stereo
pub fn sine(hz: f32, amp: f32, seconds: f32, sr: f32) -> Vec<f32> {
    (0..(seconds * sr) as usize)
        .flat_map(|i| {
            let x = (TAU * hz * i as f32 / sr).sin() * amp;
            [x, x]
        })
        .collect()
}

/// How much of a signal is *not* at the frequencies in `wanted` (each ± `tol` Hz), in dB
/// below the whole: −60 is spotless, −20 is clearly distorted
pub fn dirt_db(mono: &[f32], sr: f32, wanted: &[f32], tol: f32) -> f32 {
    let power = spectrum(mono, 8192);
    let hz = sr * 0.5 / power.len() as f32;
    let (mut clean, mut dirt) = (0.0f32, 0.0f32);
    for (i, p) in power.iter().enumerate().skip(1) {
        let f = i as f32 * hz;
        if wanted.iter().any(|w| (f - w).abs() <= tol) {
            clean += p;
        } else {
            dirt += p;
        }
    }
    10.0 * (dirt / (clean + dirt).max(1e-20)).max(1e-12).log10()
}

impl Heard {
    /// One line for the reports
    pub fn line(&self) -> String {
        let bands: Vec<String> = BANDS
            .iter()
            .zip(self.bands)
            .map(|((name, _), d)| format!("{name} {d:5.1}"))
            .collect();
        format!(
            "peak {:4.2}  {:6.1} LUFS  rms {:6.1} dB  crest {:4.1} dB  width {:4.2}  dc {:.4} | {}",
            self.peak,
            self.lufs,
            self.rms_db,
            self.crest_db,
            self.width,
            self.dc,
            bands.join("  ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44_100.0;

    #[test]
    fn a_full_scale_sine_measures_like_the_book_says() {
        // 997 Hz at full scale: −3.01 dB RMS, and in both channels 0 LUFS by definition
        let tone = sine(997.0, 1.0, 2.0, SR);
        let h = hear(&tone, SR);
        assert!((h.rms_db + 3.01).abs() < 0.05, "{}", h.rms_db);
        assert!(h.lufs.abs() < 0.4, "{}", h.lufs);
        assert!((h.crest_db - 3.01).abs() < 0.05);
        assert!(h.width > 0.999 && h.dc < 1e-3);
        // … and all of it sits in the mids
        assert!(h.bands[3] > -0.1, "{:?}", h.bands);
    }

    #[test]
    fn the_spectrum_finds_a_tone_and_its_dirt() {
        let tone = mono(&sine(1_000.0, 0.5, 1.0, SR));
        let power = spectrum(&tone, 4096);
        let top = (0..power.len()).max_by(|&a, &b| power[a].total_cmp(&power[b])).unwrap();
        assert!((top as f32 * SR / 4096.0 - 1_000.0).abs() < 12.0);
        assert!((centroid(&power, SR) - 1_000.0).abs() < 30.0);
        assert!(dirt_db(&tone, SR, &[1_000.0], 40.0) < -60.0);
        // Clipped hard, a third of it is no longer the tone
        let clipped: Vec<f32> = tone.iter().map(|x| (x * 8.0).clamp(-1.0, 1.0)).collect();
        assert!(dirt_db(&clipped, SR, &[1_000.0], 40.0) > -12.0);
    }

    #[test]
    fn low_notes_weigh_less_than_they_measure() {
        // The ear (and the K-weighting) barely counts a 30 Hz sub
        let sub = hear(&sine(30.0, 0.5, 2.0, SR), SR);
        let mid = hear(&sine(2_500.0, 0.5, 2.0, SR), SR);
        assert!((sub.rms_db - mid.rms_db).abs() < 0.1);
        assert!(mid.lufs > sub.lufs + 5.0, "{} vs {}", mid.lufs, sub.lufs);
    }
}
