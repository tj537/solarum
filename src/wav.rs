//! WAV files: 16-bit stereo PCM, written by hand (the format is tiny).

use std::io::{self, Write};
use std::path::Path;

/// A float sample as 16 bits
pub fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

#[cfg(test)]
pub fn encode(samples: &[f32], sr: u32) -> Vec<u8> {
    encode_i16(&samples.iter().map(|&s| to_i16(s)).collect::<Vec<_>>(), sr)
}

pub fn encode_i16(samples: &[i16], sr: u32) -> Vec<u8> {
    encode_channels(samples, sr, 2)
}

/// One channel (the sample library)
pub fn encode_mono_i16(samples: &[i16], sr: u32) -> Vec<u8> {
    encode_channels(samples, sr, 1)
}

fn encode_channels(samples: &[i16], sr: u32, channels: u16) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sr.to_le_bytes());
    out.extend_from_slice(&(sr * 2 * channels as u32).to_le_bytes()); // bytes per second
    out.extend_from_slice(&(2 * channels).to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &v in samples {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn write_i16(path: &Path, samples: &[i16], sr: u32) -> io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all(&encode_i16(samples, sr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, samples: &[f32], sr: u32) -> io::Result<()> {
        std::fs::write(path, encode(samples, sr))
    }

    #[test]
    fn header_is_valid() {
        let bytes = encode(&[0.0, 0.5, -1.0, 1.0], 44_100);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 44_100);
        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), 16384);
        assert_eq!(i16::from_le_bytes([bytes[48], bytes[49]]), -32767);
    }

    /// Writes one demo loop per vibe to target/snapshots/sounds/ – listen to them
    /// after changing the synth: `cargo test --release render_ -- --ignored`
    #[test]
    #[ignore]
    fn render_demo_loops() {
        use crate::song::Vibe;
        let dir = Path::new("target/snapshots/sounds");
        std::fs::create_dir_all(dir).unwrap();
        for (i, vibe) in Vibe::ALL.into_iter().enumerate() {
            let mut song = crate::gen::generate(vibe, &Default::default(), &mut crate::rng::Rng::new(100 + i as u64));
            // Let every demo show off its synth line
            song.track_mut(crate::song::Voice::Acid).mute = false;
            let buf = crate::engine::render_offline(&song, 8, 44_100, 7);
            write(
                &dir.join(format!("{}.wav", vibe.name().to_lowercase().replace(' ', "-"))),
                &buf,
                44_100,
            )
            .unwrap();
        }
        // Auto mode over two 32-bar cycles: changes, fills, two kinds of breakdown, drops
        let mut song = crate::gen::generate(Vibe::Peak, &Default::default(), &mut crate::rng::Rng::new(5));
        song.auto = true;
        song.auto_bars = 4;
        let buf = crate::engine::render_offline(&song, 68, 44_100, 3);
        write(&dir.join("auto-68-bars.wav"), &buf, 44_100).unwrap();
    }
}
