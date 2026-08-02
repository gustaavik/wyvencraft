//! The rolling list of chat lines this peer has seen.
//!
//! Lines carry their own age rather than a timestamp so the log needs no clock:
//! the state layer ticks it with the frame delta, exactly like the animation
//! clocks in the view. Age drives the closed-HUD behaviour — recent lines hang
//! over the hotbar for a few seconds and then get out of the way, while opening
//! the composer shows the whole scrollback.

use std::collections::VecDeque;

use crate::net::ChatKind;

/// How many lines the log remembers. Older ones fall off the front.
pub const MAX_LINES: usize = 100;
/// How long (s) a line stays on the HUD after arriving, with the chat closed.
pub const FADE_SECONDS: f32 = 10.0;

/// One displayed line: already formatted, because only the state layer knows
/// how to turn a `PlayerId` into a name.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatLine {
    pub kind: ChatKind,
    pub text: String,
    /// Seconds since this line arrived.
    pub age: f32,
}

/// A bounded history of chat lines.
#[derive(Debug, Clone, Default)]
pub struct ChatLog {
    lines: VecDeque<ChatLine>,
}

impl ChatLog {
    pub fn push(&mut self, kind: ChatKind, text: impl Into<String>) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(ChatLine {
            kind,
            text: text.into(),
            age: 0.0,
        });
    }

    /// Advance every line's age by the frame delta.
    pub fn tick(&mut self, dt: f32) {
        for line in &mut self.lines {
            // Saturate rather than growing without bound over a long session:
            // past the fade window the exact age stops mattering.
            line.age = (line.age + dt).min(FADE_SECONDS * 2.0);
        }
    }

    /// Every line, oldest first — what the open composer shows.
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &ChatLine> {
        self.lines.iter()
    }

    /// Only the lines still inside the fade window — what the closed HUD shows.
    pub fn recent(&self) -> impl DoubleEndedIterator<Item = &ChatLine> {
        self.lines.iter().filter(|line| line.age < FADE_SECONDS)
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long session must not grow the log forever; the oldest lines go first.
    #[test]
    fn the_log_is_bounded_and_drops_the_oldest_first() {
        let mut log = ChatLog::default();
        for i in 0..MAX_LINES + 10 {
            log.push(ChatKind::Player, format!("line {i}"));
        }
        assert_eq!(log.len(), MAX_LINES);
        assert_eq!(log.lines().next().unwrap().text, "line 10");
        assert_eq!(
            log.lines().next_back().unwrap().text,
            format!("line {}", MAX_LINES + 9)
        );
    }

    /// The HUD hides old lines but the scrollback keeps them — that split is the
    /// whole reason lines carry an age.
    #[test]
    fn faded_lines_leave_the_hud_but_stay_in_the_scrollback() {
        let mut log = ChatLog::default();
        log.push(ChatKind::System, "old");
        log.tick(FADE_SECONDS + 1.0);
        log.push(ChatKind::Player, "new");

        let recent: Vec<&str> = log.recent().map(|l| l.text.as_str()).collect();
        assert_eq!(recent, ["new"], "only the fresh line is on the HUD");
        let all: Vec<&str> = log.lines().map(|l| l.text.as_str()).collect();
        assert_eq!(all, ["old", "new"], "both survive in the scrollback");
    }

    /// Age is capped so a session left running for hours can't degrade f32
    /// precision on the fade comparison.
    #[test]
    fn age_saturates_rather_than_growing_without_bound() {
        let mut log = ChatLog::default();
        log.push(ChatKind::Player, "hello");
        for _ in 0..1000 {
            log.tick(10.0);
        }
        assert_eq!(log.lines().next().unwrap().age, FADE_SECONDS * 2.0);
    }
}
