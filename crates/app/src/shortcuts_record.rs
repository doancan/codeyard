//! Recording a key for a command on the Keyboard Shortcuts page. The key presses come from the window's own key events
//! (`App::intercept_keystrokes`, before any shortcut acts), so they are spelled the way the keymap matches them, on
//! any keyboard layout: a Turkish Q keyboard's `ö` is `ö`.

use gpui::Keystroke;

/// A chord can be followed by more (`cmd-k cmd-i`); this many are recorded at most.
pub const MAX_CHORDS: usize = 3;

/// Keys that are only modifiers: pressing them alone records nothing.
const MODIFIER_KEYS: [&str; 7] = ["shift", "control", "alt", "platform", "function", "cmd", "super"];

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Recording goes on (also when the press was ignored).
    Continue,
    /// Enter after at least one chord: these keys.
    Done(String),
    /// Escape.
    Cancel,
    /// A printable key on its own as the first key: bound with no context it would swallow that key whenever you type.
    /// Recording goes on; the page says why nothing was taken.
    Refused,
}

/// A key that types text: one character (a letter, digit or punctuation), or the space bar.
fn types_text(key: &str) -> bool {
    key == "space" || key.chars().count() == 1
}

#[derive(Debug, Default, Clone)]
pub struct Recorder {
    chords: Vec<Keystroke>,
}

impl Recorder {
    /// Takes one key press. Escape cancels; Enter ends the recording once there is a chord (as the first key it is the
    /// key Enter); modifiers alone are ignored; chords past `MAX_CHORDS` are ignored.
    pub fn press(&mut self, key: &Keystroke) -> Step {
        let bare = !key.modifiers.modified();
        if MODIFIER_KEYS.contains(&key.key.as_str()) {
            return Step::Continue;
        }
        if bare && key.key == "escape" {
            return Step::Cancel;
        }
        if bare && key.key == "enter" && !self.chords.is_empty() {
            return Step::Done(self.keys());
        }
        // Ctrl, Alt or Cmd make it a shortcut (also Ctrl+Alt, which is how AltGr reaches the app on Windows); Shift alone does not.
        let shortcut = key.modifiers.control || key.modifiers.alt || key.modifiers.platform || key.modifiers.function;
        if self.chords.is_empty() && !shortcut && types_text(&key.key) {
            return Step::Refused;
        }
        if self.chords.len() < MAX_CHORDS {
            self.chords.push(Keystroke { key: key.key.clone(), modifiers: key.modifiers, key_char: None });
        }
        Step::Continue
    }

    /// The chords so far as the file spells them ("cmd-k cmd-i"); empty before the first.
    pub fn keys(&self) -> String {
        self.chords.iter().map(Keystroke::unparse).collect::<Vec<_>>().join(" ")
    }

    /// The chords so far as the page shows them ("⌘K ⌘I").
    pub fn display(&self) -> String {
        self.chords.iter().map(crate::commands::format_keystroke).collect::<Vec<_>>().join(" ")
    }

    pub fn is_empty(&self) -> bool {
        self.chords.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(r: &mut Recorder, key: &str) -> Step {
        r.press(&Keystroke::parse(key).unwrap())
    }

    #[test]
    fn a_chord_then_enter_gives_the_keys() {
        let mut r = Recorder::default();
        assert_eq!(press(&mut r, "cmd-shift-d"), Step::Continue);
        assert_eq!(r.keys(), "cmd-shift-d");
        assert_eq!(press(&mut r, "enter"), Step::Done("cmd-shift-d".into()));
    }

    #[test]
    fn two_chords_make_a_sequence() {
        let mut r = Recorder::default();
        press(&mut r, "cmd-k");
        press(&mut r, "cmd-i");
        assert_eq!(r.keys(), "cmd-k cmd-i");
        assert_eq!(press(&mut r, "enter"), Step::Done("cmd-k cmd-i".into()));
        assert_eq!(crate::shortcuts_page::edit::canonical_keys(&r.keys()).as_deref(), Some("cmd-k cmd-i"));
    }

    #[test]
    fn escape_cancels_at_any_point() {
        let mut r = Recorder::default();
        assert_eq!(press(&mut r, "escape"), Step::Cancel);
        press(&mut r, "cmd-k");
        assert_eq!(press(&mut r, "escape"), Step::Cancel);
    }

    #[test]
    fn enter_as_the_first_key_is_recorded_and_a_modified_enter_is_a_chord() {
        let mut r = Recorder::default();
        assert_eq!(press(&mut r, "enter"), Step::Continue);
        assert_eq!(r.keys(), "enter");
        let mut r = Recorder::default();
        press(&mut r, "cmd-k");
        press(&mut r, "cmd-enter");
        assert_eq!(r.keys(), "cmd-k cmd-enter");
        let mut r = Recorder::default();
        assert_eq!(press(&mut r, "shift-escape"), Step::Continue, "only a bare Escape cancels");
        assert_eq!(r.keys(), "shift-escape");
    }

    #[test]
    fn a_printable_key_on_its_own_is_refused_as_the_first_key_only() {
        let mut r = Recorder::default();
        for key in ["a", "5", ";", "ö", "space", "shift-a", "shift-1"] {
            assert_eq!(press(&mut r, key), Step::Refused, "{key}");
        }
        assert!(r.is_empty(), "nothing was taken");
        // Named keys, function keys and anything with Ctrl, Alt or Cmd are fine, also Ctrl+Alt (AltGr on Windows).
        for key in ["f5", "tab", "enter", "left", "backspace", "ctrl-a", "alt-a", "cmd-a", "ctrl-alt-a", "ctrl-alt-shift-ö"] {
            let mut r = Recorder::default();
            assert_eq!(press(&mut r, key), Step::Continue, "{key}");
            assert!(!r.is_empty(), "{key}");
        }
        // After a first chord a bare key is the second key of a sequence.
        let mut r = Recorder::default();
        press(&mut r, "cmd-k");
        assert_eq!(press(&mut r, "a"), Step::Continue);
        assert_eq!(r.keys(), "cmd-k a");
    }

    #[test]
    fn a_modifier_alone_records_nothing() {
        let mut r = Recorder::default();
        for key in ["shift", "control", "alt", "platform", "function"] {
            assert_eq!(r.press(&Keystroke { key: key.into(), modifiers: Default::default(), key_char: None }), Step::Continue);
        }
        assert!(r.is_empty());
    }

    #[test]
    fn keys_of_other_layouts_are_kept_as_typed() {
        let mut r = Recorder::default();
        press(&mut r, "ctrl-ö");
        press(&mut r, "ç");
        assert_eq!(r.keys(), "ctrl-ö ç");
        // Typed text of the key does not change the spelling (the keymap ignores it).
        let mut r = Recorder::default();
        r.press(&Keystroke { key: "ğ".into(), modifiers: gpui::Modifiers::command(), key_char: Some("Ğ".into()) });
        assert_eq!(r.keys(), "cmd-ğ");
    }

    #[test]
    fn at_most_max_chords_are_taken() {
        let mut r = Recorder::default();
        for key in ["cmd-a", "b", "c", "d"] {
            press(&mut r, key);
        }
        assert_eq!(r.keys(), "cmd-a b c");
    }

    #[test]
    fn what_is_recorded_matches_when_pressed_again() {
        // The spelling written to the file binds exactly the key that was pressed.
        for typed in ["cmd-shift-p", "ctrl-alt-ö", "alt-cmd-down", "f13", "ctrl-;", "cmd-\\"] {
            let mut r = Recorder::default();
            let pressed = Keystroke::parse(typed).unwrap();
            r.press(&pressed);
            let binding = gpui::KeyBinding::load(&r.keys(), Box::new(gpui::NoAction), None, false, None, &gpui::DummyKeyboardMapper).unwrap();
            assert_eq!(binding.match_keystrokes(&[pressed]), Some(false), "{typed} -> {}", r.keys());
        }
    }
}
