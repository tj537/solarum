//! Playing the user's own sounds: one voice per track and the library's audition.
//! Runs on the audio thread – it only clones and drops `Arc`s the UI also holds, so nothing is freed here.

use std::sync::Arc;

/// `Track::sound` from here on is one of the project's sample slots
pub const CUSTOM: u8 = 200;
/// How many of your own sounds a project can use at once (more than anyone needs; the most
/// serde and `Default` handle as an array)
pub const SAMPLE_SLOTS: usize = 32;
/// Everything in the library is kept at this rate
pub const SAMPLE_SR: f32 = 44_100.0;

/// How loud your sounds play on each track (a normalized sound peaks at 0.89): about where the
/// built-in sounds of the track peak
pub const LEVEL: [f32; 8] = [0.62, 0.6, 0.6, 0.55, 0.5, 0.45, 0.6, 0.5];

/// A sound ready to play: the edited audio (mono, `SAMPLE_SR`) and its pitch in semitones
pub struct SampleBuf {
    pub data: Vec<f32>,
    pub pitch: f32,
}

pub type Slots = [Option<Arc<SampleBuf>>; SAMPLE_SLOTS];

/// The slot a track's sound points at, if it is one of yours
pub fn slot_of(sound: u8) -> Option<usize> {
    let k = sound.checked_sub(CUSTOM)? as usize;
    (k < SAMPLE_SLOTS).then_some(k)
}

#[derive(Clone, Default)]
struct Play {
    buf: Option<Arc<SampleBuf>>,
    pos: f64,
    rate: f64,
    amp: f32,
    /// Samples (output rate) until the release starts
    hold: f32,
    env: f32,
    release: f32,
}

impl Play {
    fn next(&mut self) -> f32 {
        let Some(buf) = &self.buf else { return 0.0 };
        let i = self.pos as usize;
        if i + 1 >= buf.data.len() || self.env < 1e-4 {
            self.buf = None;
            return 0.0;
        }
        let f = (self.pos - i as f64) as f32;
        let x = buf.data[i] * (1.0 - f) + buf.data[i + 1] * f;
        self.pos += self.rate;
        if self.hold > 0.0 {
            self.hold -= 1.0;
        } else {
            self.env *= self.release;
        }
        x * self.amp * self.env
    }
}

/// One track's sampler: the note playing, and the one before it fading out (no clicks on a retrigger)
#[derive(Clone, Default)]
pub struct Sampler {
    now: Play,
    old: Play,
    fade: f32,
}

impl Sampler {
    /// `semis` above the sound's own pitch; `hold` in output samples (`None`: the whole sound)
    pub fn trigger(&mut self, buf: &Arc<SampleBuf>, semis: f32, vel: f32, hold: Option<f32>, sr: f32) {
        self.old = std::mem::take(&mut self.now);
        self.fade = 1.0;
        let rate = 2f64.powf((semis + buf.pitch) as f64 / 12.0) * (SAMPLE_SR / sr) as f64;
        self.now = Play {
            buf: Some(buf.clone()),
            pos: 0.0,
            rate,
            amp: vel,
            hold: hold.unwrap_or(f32::MAX),
            env: 1.0,
            release: (-1.0 / (0.03 * sr)).exp(),
        };
    }

    pub fn active(&self) -> bool {
        self.now.buf.is_some() || self.old.buf.is_some()
    }

    pub fn next(&mut self) -> f32 {
        let mut x = self.now.next();
        if self.old.buf.is_some() {
            self.fade = (self.fade - 1.0 / 200.0).max(0.0);
            x += self.old.next() * self.fade;
            if self.fade == 0.0 {
                self.old.buf = None;
            }
        }
        x
    }

    /// Playback position of the sound playing now, in `SAMPLE_SR` samples
    pub fn position(&self) -> Option<f64> {
        self.now.buf.as_ref().map(|_| self.now.pos)
    }
}

/// The library's audition: plays a sound once or looped, whether the music runs or not
#[derive(Default)]
pub struct Audition {
    pub voice: Sampler,
    pub looped: Option<Arc<SampleBuf>>,
}

impl Audition {
    pub fn next(&mut self, sr: f32) -> f32 {
        if !self.voice.active() {
            if let Some(b) = &self.looped {
                let b = b.clone();
                self.voice.trigger(&b, 0.0, 1.0, None, sr);
            }
        }
        self.voice.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_the_sound_and_ends() {
        let buf = Arc::new(SampleBuf {
            data: (0..1000).map(|i| (i as f32 * 0.1).sin()).collect(),
            pitch: 0.0,
        });
        let mut s = Sampler::default();
        s.trigger(&buf, 0.0, 1.0, None, SAMPLE_SR);
        let out: Vec<f32> = (0..1200).map(|_| s.next()).collect();
        assert!(out[..900].iter().any(|x| x.abs() > 0.5));
        assert!(out[1000..].iter().all(|&x| x == 0.0));
        // An octave up takes half the time
        s.trigger(&buf, 12.0, 1.0, None, SAMPLE_SR);
        let n = (0..2000).take_while(|_| s.next() != 0.0 || s.active()).count();
        assert!((480..=520).contains(&n), "{n}");
    }

    #[test]
    fn slots_are_the_custom_range() {
        assert_eq!(slot_of(0), None);
        assert_eq!(slot_of(CUSTOM), Some(0));
        assert_eq!(slot_of(CUSTOM + 31), Some(31));
        assert_eq!(slot_of(CUSTOM + 32), None);
    }
}
