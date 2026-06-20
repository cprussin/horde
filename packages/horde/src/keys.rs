//! Encode crossterm key events into the terminal byte sequences a focused
//! session's PTY program (claude/Ink) expects on stdin.
//!
//! Legacy xterm profile (no Kitty keyboard protocol).  This is the single most
//! fidelity-sensitive part of the multiplexer, so it lives here in isolation
//! with an exhaustive test table.  The blur key (Ctrl-B) is intercepted by the
//! app before this is called, so it is never encoded/forwarded.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encode one key press into the bytes to send to the session.
pub fn encode(key: &KeyEvent) -> Vec<u8> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let mut bytes = match key.code {
        KeyCode::Char(c) => return encode_char(c, key.modifiers),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => return b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => return csi_final(b'A', key.modifiers),
        KeyCode::Down => return csi_final(b'B', key.modifiers),
        KeyCode::Right => return csi_final(b'C', key.modifiers),
        KeyCode::Left => return csi_final(b'D', key.modifiers),
        KeyCode::Home => return csi_final(b'H', key.modifiers),
        KeyCode::End => return csi_final(b'F', key.modifiers),
        KeyCode::PageUp => return csi_tilde(5, key.modifiers),
        KeyCode::PageDown => return csi_tilde(6, key.modifiers),
        KeyCode::Insert => return csi_tilde(2, key.modifiers),
        KeyCode::Delete => return csi_tilde(3, key.modifiers),
        KeyCode::F(n) => return function_key(n),
        _ => return Vec::new(),
    };
    // Alt on the simple keys above prefixes ESC.
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// Wrap pasted text in bracketed-paste markers so it isn't interpreted as
/// keystrokes (matters for multi-line pastes into claude's prompt).
pub fn encode_paste(text: &str) -> Vec<u8> {
    let mut v = b"\x1b[200~".to_vec();
    v.extend_from_slice(text.as_bytes());
    v.extend_from_slice(b"\x1b[201~");
    v
}

fn encode_char(c: char, m: KeyModifiers) -> Vec<u8> {
    let mut bytes = Vec::new();
    if m.contains(KeyModifiers::CONTROL) {
        match ctrl_byte(c) {
            Some(b) => bytes.push(b),
            // Ctrl+<non-mappable> (digits, most punctuation): send the char.
            None => push_char(&mut bytes, c),
        }
    } else {
        // crossterm already resolved Shift (e.g. 'A'), so don't reapply it.
        push_char(&mut bytes, c);
    }
    if m.contains(KeyModifiers::ALT) {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn push_char(bytes: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// The control byte for Ctrl+<c>, for the canonical range (letters and
/// `@ [ \ ] ^ _` and space); `None` otherwise.
fn ctrl_byte(c: char) -> Option<u8> {
    if c.is_ascii_alphabetic() {
        return Some((c.to_ascii_uppercase() as u8) & 0x1f);
    }
    match c {
        ' ' | '@' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        _ => None,
    }
}

/// A CSI sequence ending in `final_byte` (arrows, Home/End), with the xterm
/// modifier encoding when any modifier is held.
fn csi_final(final_byte: u8, m: KeyModifiers) -> Vec<u8> {
    match modifier_param(m) {
        None => vec![0x1b, b'[', final_byte],
        Some(p) => {
            let mut v = format!("\x1b[1;{p}").into_bytes();
            v.push(final_byte);
            v
        }
    }
}

/// A `CSI <n> ~` sequence (PageUp/Down, Insert, Delete), with modifiers.
fn csi_tilde(n: u8, m: KeyModifiers) -> Vec<u8> {
    match modifier_param(m) {
        None => format!("\x1b[{n}~").into_bytes(),
        Some(p) => format!("\x1b[{n};{p}~").into_bytes(),
    }
}

fn function_key(n: u8) -> Vec<u8> {
    match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => Vec::new(),
    }
}

/// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4); `None` when none.
fn modifier_param(m: KeyModifiers) -> Option<u8> {
    let mut v = 1u8;
    if m.contains(KeyModifiers::SHIFT) {
        v += 1;
    }
    if m.contains(KeyModifiers::ALT) {
        v += 2;
    }
    if m.contains(KeyModifiers::CONTROL) {
        v += 4;
    }
    (v != 1).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn printable_and_shift() {
        assert_eq!(encode(&ev(KeyCode::Char('a'), KeyModifiers::NONE)), b"a");
        // crossterm resolves shift into the char; don't double-apply.
        assert_eq!(encode(&ev(KeyCode::Char('A'), KeyModifiers::SHIFT)), b"A");
        assert_eq!(
            encode(&ev(KeyCode::Char('é'), KeyModifiers::NONE)),
            "é".as_bytes()
        );
    }

    #[test]
    fn control_combos() {
        assert_eq!(
            encode(&ev(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            [0x01]
        );
        assert_eq!(
            encode(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            [0x03]
        );
        assert_eq!(
            encode(&ev(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            [0x02]
        );
        assert_eq!(
            encode(&ev(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            [0x00]
        );
        assert_eq!(
            encode(&ev(KeyCode::Char('['), KeyModifiers::CONTROL)),
            [0x1b]
        );
        assert_eq!(
            encode(&ev(KeyCode::Char('\\'), KeyModifiers::CONTROL)),
            [0x1c]
        );
        // Non-mappable ctrl combo falls back to the char.
        assert_eq!(encode(&ev(KeyCode::Char('1'), KeyModifiers::CONTROL)), b"1");
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(encode(&ev(KeyCode::Char('x'), KeyModifiers::ALT)), b"\x1bx");
        assert_eq!(encode(&ev(KeyCode::Enter, KeyModifiers::ALT)), b"\x1b\r");
    }

    #[test]
    fn named_keys() {
        assert_eq!(encode(&ev(KeyCode::Enter, KeyModifiers::NONE)), b"\r");
        assert_eq!(encode(&ev(KeyCode::Tab, KeyModifiers::NONE)), b"\t");
        assert_eq!(
            encode(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)),
            b"\x1b[Z"
        );
        assert_eq!(encode(&ev(KeyCode::Backspace, KeyModifiers::NONE)), [0x7f]);
        assert_eq!(encode(&ev(KeyCode::Esc, KeyModifiers::NONE)), [0x1b]);
    }

    #[test]
    fn arrows_and_modifiers() {
        assert_eq!(encode(&ev(KeyCode::Up, KeyModifiers::NONE)), b"\x1b[A");
        assert_eq!(encode(&ev(KeyCode::Left, KeyModifiers::NONE)), b"\x1b[D");
        assert_eq!(encode(&ev(KeyCode::Up, KeyModifiers::SHIFT)), b"\x1b[1;2A");
        assert_eq!(
            encode(&ev(KeyCode::Right, KeyModifiers::CONTROL)),
            b"\x1b[1;5C"
        );
        assert_eq!(encode(&ev(KeyCode::Left, KeyModifiers::ALT)), b"\x1b[1;3D");
    }

    #[test]
    fn home_end_nav() {
        assert_eq!(encode(&ev(KeyCode::Home, KeyModifiers::NONE)), b"\x1b[H");
        assert_eq!(encode(&ev(KeyCode::End, KeyModifiers::NONE)), b"\x1b[F");
        assert_eq!(
            encode(&ev(KeyCode::Home, KeyModifiers::CONTROL)),
            b"\x1b[1;5H"
        );
        assert_eq!(encode(&ev(KeyCode::PageUp, KeyModifiers::NONE)), b"\x1b[5~");
        assert_eq!(
            encode(&ev(KeyCode::PageDown, KeyModifiers::NONE)),
            b"\x1b[6~"
        );
        assert_eq!(encode(&ev(KeyCode::Delete, KeyModifiers::NONE)), b"\x1b[3~");
        assert_eq!(encode(&ev(KeyCode::Insert, KeyModifiers::NONE)), b"\x1b[2~");
        assert_eq!(
            encode(&ev(KeyCode::Delete, KeyModifiers::SHIFT)),
            b"\x1b[3;2~"
        );
    }

    #[test]
    fn function_keys() {
        assert_eq!(encode(&ev(KeyCode::F(1), KeyModifiers::NONE)), b"\x1bOP");
        assert_eq!(encode(&ev(KeyCode::F(4), KeyModifiers::NONE)), b"\x1bOS");
        assert_eq!(encode(&ev(KeyCode::F(5), KeyModifiers::NONE)), b"\x1b[15~");
        assert_eq!(encode(&ev(KeyCode::F(12), KeyModifiers::NONE)), b"\x1b[24~");
    }

    #[test]
    fn paste_is_bracketed() {
        assert_eq!(encode_paste("hi\nthere"), b"\x1b[200~hi\nthere\x1b[201~");
    }
}
