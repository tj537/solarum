//! MIDI input: plays pads, transposes the bass and acid, turns the knobs.
//!
//! Every connected input is opened. Mapping, made for small keyboards:
//! - Keys below middle C play the eight sounds (C kick, D clap, E rim, F hat,
//!   G open hat, A ride, B bass, black keys fill in the gaps).
//! - Middle C and up: hold a key to transpose bass and acid live (C4 = original).
//! - Pitch bend: the filter. Mod wheel: Acid. Sustain pedal: echo throw.
//! - Knobs: CC 70–73 (Akai MPK mini default) or CC 74/71/91/93 → Energy,
//!   Darkness, Acid, Space.

use crate::engine::{Cmd, Fx};
use crate::song::Voice;
use midir::{Ignore, MidiInput, MidiInputConnection};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// What a MIDI message means for Solarum
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Midi {
    Pad(Voice, f32),
    /// Some(semitones) while a key ≥ C4 is held, None when released
    Transpose(Option<i8>),
    Filter(f32),
    Macro(usize, f32),
    Echo(bool),
}

pub fn pad_for(note: u8) -> Voice {
    match note % 12 {
        0 => Voice::Kick,
        1 => Voice::Rim,
        2 => Voice::Clap,
        3 => Voice::Hat,
        4 => Voice::Rim,
        5 => Voice::Hat,
        6 => Voice::Hat,
        7 => Voice::OpenHat,
        8 => Voice::OpenHat,
        9 => Voice::Ride,
        10 => Voice::Ride,
        _ => Voice::Bass,
    }
}

/// Decode one MIDI message; `held` tracks the keys that transpose
pub fn decode(msg: &[u8], held: &mut Vec<u8>) -> Option<Midi> {
    let (&status, rest) = msg.split_first()?;
    // A status byte has the top bit set and data bytes never do: anything else is a broken
    // device or port, and a note above 127 would transpose by nonsense
    if status < 0x80 || rest.iter().any(|&b| b > 0x7F) {
        return None;
    }
    let kind = status & 0xF0;
    match (kind, rest) {
        (0x90, &[note, vel]) if vel > 0 => {
            if note < 60 {
                Some(Midi::Pad(pad_for(note), (vel as f32 / 127.0).max(0.2)))
            } else {
                held.retain(|&n| n != note);
                held.push(note);
                Some(Midi::Transpose(Some(note as i8 - 60)))
            }
        }
        (0x80, &[note, _]) | (0x90, &[note, 0]) => {
            if note < 60 {
                return None;
            }
            held.retain(|&n| n != note);
            Some(Midi::Transpose(held.last().map(|&n| n as i8 - 60)))
        }
        (0xE0, &[lsb, msb]) => {
            let v = ((msb as i32) << 7 | lsb as i32) - 8192;
            Some(Midi::Filter(v as f32 / 8192.0))
        }
        (0xB0, &[cc, val]) => {
            let v = val as f32 / 127.0;
            match cc {
                1 => Some(Midi::Macro(2, v)),
                64 => Some(Midi::Echo(val >= 64)),
                70 | 74 => Some(Midi::Macro(0, v)),
                71 => Some(Midi::Macro(1, v)),
                72 | 91 => Some(Midi::Macro(2, v)),
                73 | 93 => Some(Midi::Macro(3, v)),
                _ => None,
            }
        }
        _ => None,
    }
}

pub struct MidiIn {
    connections: Vec<MidiInputConnection<()>>,
    pub names: Vec<String>,
}

impl MidiIn {
    /// Connect to every input port. Sounds, transpose, filter and echo go
    /// straight to the engine (no waiting for the next UI frame); everything is
    /// also reported to the UI on `ui`.
    pub fn connect(engine: &Sender<Cmd>, ui: &Sender<Midi>, record: Arc<AtomicBool>, wake: impl Fn() + Clone + Send + 'static) -> Self {
        let mut connections = Vec::new();
        let mut names = Vec::new();
        let Ok(probe) = MidiInput::new("Solarum") else {
            return MidiIn { connections, names };
        };
        for port in probe.ports() {
            let Ok(mut input) = MidiInput::new("Solarum") else { continue };
            input.ignore(Ignore::All);
            let name = probe.port_name(&port).unwrap_or_else(|_| "MIDI".into());
            if name.to_lowercase().contains("through") {
                continue;
            }
            let (engine, ui, record, wake) = (engine.clone(), ui.clone(), record.clone(), wake.clone());
            let mut held = Vec::new();
            if let Ok(conn) = input.connect(
                &port,
                "solarum-in",
                move |_, msg, _| {
                    let Some(m) = decode(msg, &mut held) else { return };
                    let cmd = match m {
                        Midi::Pad(voice, vel) => Some(Cmd::Hit {
                            voice,
                            vel,
                            record: record.load(Ordering::Relaxed),
                        }),
                        Midi::Transpose(t) => Some(Cmd::Transpose(t.unwrap_or(0))),
                        Midi::Filter(x) => Some(Cmd::Filter(x)),
                        Midi::Echo(on) => Some(Cmd::Fx(Fx::Echo, on)),
                        Midi::Macro(..) => None, // changes the song – the UI does that
                    };
                    if let Some(cmd) = cmd {
                        let _ = engine.send(cmd);
                    }
                    let _ = ui.send(m);
                    wake();
                },
                (),
            ) {
                names.push(name);
                connections.push(conn);
            }
        }
        MidiIn { connections, names }
    }

    pub fn port_count() -> usize {
        MidiInput::new("Solarum probe").map(|m| m.port_count()).unwrap_or(0)
    }

    pub fn is_connected(&self) -> bool {
        !self.connections.is_empty()
    }

    pub fn close(self) {
        for c in self.connections {
            c.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_below_middle_c_are_pads() {
        let mut held = Vec::new();
        assert_eq!(decode(&[0x90, 48, 100], &mut held), Some(Midi::Pad(Voice::Kick, 100.0 / 127.0)));
        assert_eq!(decode(&[0x91, 50, 127], &mut held), Some(Midi::Pad(Voice::Clap, 1.0)));
        assert_eq!(decode(&[0x80, 50, 0], &mut held), None);
    }

    #[test]
    fn upper_keys_transpose_while_held() {
        let mut held = Vec::new();
        assert_eq!(decode(&[0x90, 63, 90], &mut held), Some(Midi::Transpose(Some(3))));
        assert_eq!(decode(&[0x90, 67, 90], &mut held), Some(Midi::Transpose(Some(7))));
        // Releasing the newest key goes back to the one still held
        assert_eq!(decode(&[0x80, 67, 0], &mut held), Some(Midi::Transpose(Some(3))));
        // Note-on with velocity 0 is a note-off
        assert_eq!(decode(&[0x90, 63, 0], &mut held), Some(Midi::Transpose(None)));
    }

    #[test]
    fn broken_bytes_are_ignored() {
        let mut held = Vec::new();
        assert_eq!(decode(&[0x90, 200, 100], &mut held), None);
        assert_eq!(decode(&[0x90, 64, 0xFF], &mut held), None);
        assert_eq!(decode(&[0x40, 64, 100], &mut held), None);
        assert!(held.is_empty());
    }

    #[test]
    fn controllers() {
        let mut held = Vec::new();
        assert_eq!(decode(&[0xE0, 0, 64], &mut held), Some(Midi::Filter(0.0)));
        assert_eq!(decode(&[0xB0, 1, 127], &mut held), Some(Midi::Macro(2, 1.0)));
        assert_eq!(decode(&[0xB0, 64, 127], &mut held), Some(Midi::Echo(true)));
        assert_eq!(decode(&[0xB0, 73, 0], &mut held), Some(Midi::Macro(3, 0.0)));
        assert_eq!(decode(&[0xF8], &mut held), None); // clock
    }
}
