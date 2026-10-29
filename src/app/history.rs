//! Undo and redo of song edits (the undo steps themselves are recorded in finish_frame).

use super::*;

impl SolarumApp {
    pub(super) fn undo(&mut self, now: f64) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.song);
            self.song = keep_live_settings(prev, &self.song);
            self.restoring = true;
            self.slot_now = None;
            self.say("Undo", now);
        }
    }

    pub(super) fn redo(&mut self, now: f64) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.song);
            self.song = keep_live_settings(next, &self.song);
            self.restoring = true;
            self.slot_now = None;
            self.say("Redo", now);
        }
    }
}
