//! Devices: MIDI input, the audio output and the check every few seconds for plugged or unplugged ones.

use super::*;

impl SolarumApp {
    pub(super) fn connect_midi(&mut self, wake: impl Fn() + Clone + Send + 'static) {
        if let Some(m) = self.midi.take() {
            m.close();
        }
        self.midi_ports = MidiIn::port_count();
        let m = MidiIn::connect(&self.tx, &self.midi_tx, self.record.clone(), wake);
        if !m.names.is_empty() {
            self.say(format!("{} connected", m.names.join(", ")), 0.0);
        }
        self.midi = Some(m);
    }

    /// Start (or restart) the audio output – after the device changed or failed
    pub(super) fn restart_audio(&mut self, ctx: &egui::Context, now: f64) {
        let was_playing = self.playing();
        let (tx, rx) = channel();
        let (ev_tx, events) = channel();
        self.audio_failed.store(false, Ordering::Relaxed);
        self.audio = Err("Reconnecting…".into()); // close the old stream first
        self.audio = audio::start(self.song, rx, ev_tx, self.shared.clone(), self.audio_failed.clone());
        self.songs_sent = 0;
        self.tx = tx;
        self.events = events;
        self.sent = self.song;
        self.samples_sent = Default::default();
        self.fx_sent = [false; 6];
        self.audio_device = audio::default_device_name();
        if was_playing {
            self.send(Cmd::Play);
        }
        if self.filter != 0.0 {
            self.send(Cmd::Filter(self.filter));
        }
        self.send_arrangement();
        // A session recording goes on in the new engine
        if self.capture {
            self.send(Cmd::Capture(true));
        }
        // MIDI talks to the engine directly – reconnect it to the new one
        let c = ctx.clone();
        self.connect_midi(move || c.request_repaint());
        match &self.audio {
            Ok(a) => {
                let name = a.device.clone();
                self.say(format!("Sound now on {name}"), now);
            }
            Err(e) => {
                let e = e.clone();
                self.say(format!("No sound: {e}"), now);
            }
        }
    }

    /// MIDI keyboard or headphones plugged in or out? Check every few seconds
    pub(super) fn check_devices(&mut self, ctx: &egui::Context, now: f64) {
        if now > self.midi_scan_at && self.with_devices {
            self.midi_scan_at = now + 3.0;
            if MidiIn::port_count() != self.midi_ports {
                let c = ctx.clone();
                self.connect_midi(move || c.request_repaint());
            }
            let device = audio::default_device_name();
            let broken = self.shared.broken.swap(false, Ordering::Relaxed);
            if broken || self.audio_failed.load(Ordering::Relaxed) || (device.is_some() && device != self.audio_device) {
                self.restart_audio(ctx, now);
            }
        }
        if self.with_devices {
            // Keep the 3-second checks running even when nothing moves
            ctx.request_repaint_after(std::time::Duration::from_secs(3));
        }
    }
}
