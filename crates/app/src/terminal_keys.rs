//! What a key sends to the program in the terminal: the bytes of a keystroke in the xterm and readline conventions, and the
//! macOS line-editing keys.

use alacritty_terminal::term::TermMode;

/// macOS line editing, as in Terminal.app: ⌘← / ⌘→ go to the start / end of the line, ⌥← / ⌥→ move by word,
/// ⌘⌫ deletes to the start of the line, ⌘⌦ to its end, ⌥⌦ deletes the next word. They are the readline / zle keys
/// (Ctrl+A, Ctrl+E, Esc b, Esc f, Ctrl+U, Ctrl+K, Esc d), so they work in bash, zsh, fish and most REPLs. Full-screen programs
/// (vim, less) get the plain xterm keys instead: Ctrl+A there would mean something else.
pub fn macos_line_editing(k: &gpui::Keystroke, mode: TermMode) -> Option<&'static [u8]> {
    let m = &k.modifiers;
    if m.shift || m.control {
        return None;
    }
    // Full-screen programs get Home / End instead of readline's Ctrl+A / Ctrl+E.
    if mode.contains(TermMode::ALT_SCREEN) {
        let app = mode.contains(TermMode::APP_CURSOR);
        return match (k.key.as_str(), m.platform, m.alt) {
            ("left", true, false) => Some(if app { b"\x1bOH" } else { b"\x1b[H" }),
            ("right", true, false) => Some(if app { b"\x1bOF" } else { b"\x1b[F" }),
            _ => None,
        };
    }
    Some(match (k.key.as_str(), m.platform, m.alt) {
        ("left", true, false) => b"\x01",
        ("right", true, false) => b"\x05",
        ("backspace", true, false) => b"\x15",
        ("delete", true, false) => b"\x0b",
        ("left", false, true) => b"\x1bb",
        ("right", false, true) => b"\x1bf",
        ("delete", false, true) => b"\x1bd",
        _ => return None,
    })
}

/// Key → terminal bytes (xterm sequences). ⌘ keys are left to the app, except the macOS line editing keys.
pub fn key_to_bytes(k: &gpui::Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    if cfg!(target_os = "macos")
        && let Some(bytes) = macos_line_editing(k, mode)
    {
        return Some(bytes.to_vec());
    }
    if m.platform {
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    // xterm modifier code: 1 + shift + 2·alt + 4·ctrl
    let modcode = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let csi = |final_byte: char| -> Vec<u8> {
        if modcode > 1 {
            format!("\x1b[1;{modcode}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if modcode > 1 { format!("\x1b[{n};{modcode}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    let bytes = match k.key.as_str() {
        "enter" => b"\r".to_vec(),
        "backspace" if m.alt => b"\x1b\x7f".to_vec(),
        "backspace" if m.control => b"\x08".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => csi('A'),
        "down" => csi('B'),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if m.control => vec![0],
        "space" => b" ".to_vec(),
        key if m.control && key.chars().count() == 1 => {
            let ch = key.chars().next()?.to_ascii_lowercase();
            match ch {
                'a'..='z' => vec![ch as u8 - b'a' + 1],
                '[' => vec![0x1b],
                '\\' => vec![0x1c],
                ']' => vec![0x1d],
                '/' | '_' | '-' => vec![0x1f],
                '2' | '@' => vec![0],
                _ => return None,
            }
        }
        _ => {
            // Typed character (ğ, ş... on a Turkish keyboard; includes ones produced with ⌥ on macOS).
            let typed = k.key_char.clone().or_else(|| (k.key.chars().count() == 1).then(|| k.key.clone()))?;
            // Alt+letter is the meta key of readline, emacs and vim: ESC first. Not when Alt composed another character (⌥ on
            // macOS, AltGr on a Turkish keyboard), which is typed as it is.
            let plain = k.key.chars().count() == 1 && typed.to_lowercase() == k.key;
            let mut bytes = typed.into_bytes();
            if m.alt && !m.control && !m.platform && plain {
                bytes.insert(0, 0x1b);
            }
            bytes
        }
    };
    Some(bytes)
}
