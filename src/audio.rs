//! Audio output with cpal: the engine runs inside the device callback.

use crate::engine::{Cmd, Engine, Event, Shared};
use crate::song::Song;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// The most frames the engine renders at once
const CHUNK: usize = 8192;

pub struct Audio {
    _stream: Stream,
    pub sample_rate: u32,
    pub device: String,
}

/// Name of the system's current default output (to notice headphones being plugged in)
pub fn default_device_name() -> Option<String> {
    cpal::default_host().default_output_device().and_then(|d| d.name().ok())
}

/// Open the default output and start the engine. Commands come in on `rx`,
/// song changes made by the engine go out on `events`. `failed` is set when
/// the device goes away (unplugged headphones …) so the UI can reconnect.
pub fn start(song: Song, rx: Receiver<Cmd>, events: Sender<Event>, shared: Arc<Shared>, failed: Arc<AtomicBool>) -> Result<Audio, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("No audio output found")?;
    let name = device.name().unwrap_or_else(|_| "Audio output".into());
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let sr = config.sample_rate().0;
    let channels = config.channels() as usize;
    let format = config.sample_format();
    let mut stream_config: cpal::StreamConfig = config.into();
    // The device's default buffer: low latency without crackles on most systems
    stream_config.buffer_size = cpal::BufferSize::Default;

    let mut engine = Engine::new(sr as f32, song, shared, Some(events), seed());
    // Allocated once here: the callback renders in pieces of at most `CHUNK` frames, so even a
    // device asking for a huge buffer never makes it allocate
    let mut scratch = vec![0.0f32; CHUNK * 2];
    let err = move |e| {
        eprintln!("audio error: {e}");
        failed.store(true, Ordering::Relaxed);
    };

    // Render stereo into `scratch`, then spread it over the device's channels
    macro_rules! build {
        ($t:ty, $conv:expr) => {
            device.build_output_stream(
                &stream_config,
                move |data: &mut [$t], _| {
                    engine.drain(&rx);
                    for block in data.chunks_mut(CHUNK * channels) {
                        let stereo = &mut scratch[..block.len() / channels * 2];
                        engine.render(stereo);
                        for (i, frame) in block.chunks_exact_mut(channels).enumerate() {
                            for (c, s) in frame.iter_mut().enumerate() {
                                let (l, r) = (stereo[i * 2], stereo[i * 2 + 1]);
                                // A mono output gets both sides, extra channels stay silent
                                let v = match (channels, c) {
                                    (1, _) => (l + r) * 0.5,
                                    (_, 0) => l,
                                    (_, 1) => r,
                                    _ => 0.0,
                                };
                                *s = $conv(v);
                            }
                        }
                    }
                },
                err,
                None,
            )
        };
    }
    let stream = match format {
        SampleFormat::F32 => build!(f32, |v: f32| v),
        SampleFormat::I16 => build!(i16, |v: f32| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
        SampleFormat::U16 => build!(u16, |v: f32| ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16),
        SampleFormat::I32 => build!(i32, |v: f32| (v.clamp(-1.0, 1.0) as f64 * i32::MAX as f64) as i32),
        other => return Err(format!("Unsupported sample format {other}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(Audio {
        _stream: stream,
        sample_rate: sr,
        device: name,
    })
}

fn seed() -> u64 {
    crate::rng::Rng::from_clock().next_u32() as u64
}
