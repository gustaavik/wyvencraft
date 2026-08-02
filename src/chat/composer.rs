//! The chat input line: what's being typed, and the history behind it.
//!
//! Kept out of the view so submitting, cancelling and walking the history are
//! testable without egui. The view owns only the widget; it reports what the
//! player did and reads `draft` back.

/// How many submitted lines the up-arrow can walk back through.
pub const MAX_HISTORY: usize = 32;

/// The chat input line's state.
#[derive(Debug, Clone, Default)]
pub struct Composer {
    /// Whether the input line is showing and holding the keyboard.
    pub open: bool,
    /// What's typed so far.
    pub draft: String,
    /// Previously submitted lines, oldest first.
    history: Vec<String>,
    /// How far back through `history` the player has walked, if at all.
    cursor: Option<usize>,
    /// Set when the line opens; the view consumes it to focus the widget once.
    focus_requested: bool,
}

impl Composer {
    /// Show the input line, pre-filled with `prefill` (`"/"` when opened with
    /// the command key, so a command is one keystroke closer).
    pub fn begin(&mut self, prefill: &str) {
        self.open = true;
        self.draft.clear();
        self.draft.push_str(prefill);
        self.cursor = None;
        self.focus_requested = true;
    }

    /// Hide the input line and discard whatever was typed.
    pub fn close(&mut self) {
        self.open = false;
        self.draft.clear();
        self.cursor = None;
        self.focus_requested = false;
    }

    /// Close the line and hand back what was typed, recording it in the history.
    /// `None` when the draft was blank — Enter on an empty line just closes it.
    pub fn submit(&mut self) -> Option<String> {
        let text = self.draft.trim().to_string();
        self.close();
        if text.is_empty() {
            return None;
        }
        // Don't stutter: repeating the last line shouldn't double it up.
        if self.history.last() != Some(&text) {
            if self.history.len() == MAX_HISTORY {
                self.history.remove(0);
            }
            self.history.push(text.clone());
        }
        Some(text)
    }

    /// Step one entry further back in the history, into the draft.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.cursor {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.cursor = Some(next);
        self.draft = self.history[next].clone();
    }

    /// Step one entry forward, back toward a blank draft.
    pub fn history_next(&mut self) {
        let Some(i) = self.cursor else { return };
        if i + 1 < self.history.len() {
            self.cursor = Some(i + 1);
            self.draft = self.history[i + 1].clone();
        } else {
            self.cursor = None;
            self.draft.clear();
        }
    }

    /// Whether the view should focus the widget this frame (consumed on read,
    /// so focus is requested once rather than fought for every frame).
    pub fn take_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.focus_requested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_with_a_prefill_starts_the_command_for_you() {
        let mut composer = Composer::default();
        composer.begin("/");
        assert!(composer.open);
        assert_eq!(composer.draft, "/");
        assert!(composer.take_focus_request(), "the widget is focused once");
        assert!(!composer.take_focus_request(), "and not again");
    }

    #[test]
    fn submitting_closes_the_line_and_records_history() {
        let mut composer = Composer::default();
        composer.begin("");
        composer.draft = "  hello  ".to_string();
        assert_eq!(composer.submit(), Some("hello".to_string()));
        assert!(!composer.open);
        assert!(composer.draft.is_empty());
    }

    /// Enter on a blank line is a cancel, not an empty message on everyone's HUD.
    #[test]
    fn an_empty_draft_submits_nothing() {
        let mut composer = Composer::default();
        composer.begin("");
        composer.draft = "   ".to_string();
        assert_eq!(composer.submit(), None);
        assert!(!composer.open);
    }

    #[test]
    fn cancelling_discards_the_draft() {
        let mut composer = Composer::default();
        composer.begin("/");
        composer.draft = "/give bread 5".to_string();
        composer.close();
        assert!(!composer.open);
        assert!(composer.draft.is_empty());
    }

    /// Retyping a long `/give` after a typo is the common case, so the history
    /// has to walk back in submission order and then return to a blank line.
    #[test]
    fn history_walks_back_through_past_messages_and_returns() {
        let mut composer = Composer::default();
        for line in ["first", "second"] {
            composer.begin("");
            composer.draft = line.to_string();
            composer.submit();
        }
        composer.begin("");

        composer.history_prev();
        assert_eq!(composer.draft, "second");
        composer.history_prev();
        assert_eq!(composer.draft, "first");
        composer.history_prev();
        assert_eq!(composer.draft, "first", "the oldest entry is the floor");

        composer.history_next();
        assert_eq!(composer.draft, "second");
        composer.history_next();
        assert_eq!(
            composer.draft, "",
            "walking past the newest clears the draft"
        );
        composer.history_next();
        assert_eq!(composer.draft, "", "and stays cleared");
    }

    #[test]
    fn repeating_a_line_does_not_duplicate_it_in_history() {
        let mut composer = Composer::default();
        for _ in 0..3 {
            composer.begin("");
            composer.draft = "/help".to_string();
            composer.submit();
        }
        composer.begin("");
        composer.history_prev();
        assert_eq!(composer.draft, "/help");
        composer.history_prev();
        assert_eq!(composer.draft, "/help", "only one entry was recorded");
    }

    #[test]
    fn the_history_is_bounded() {
        let mut composer = Composer::default();
        for i in 0..MAX_HISTORY + 5 {
            composer.begin("");
            composer.draft = format!("line {i}");
            composer.submit();
        }
        composer.begin("");
        // Walk all the way back; the oldest survivor is the floor.
        for _ in 0..MAX_HISTORY * 2 {
            composer.history_prev();
        }
        assert_eq!(composer.draft, "line 5");
    }
}
