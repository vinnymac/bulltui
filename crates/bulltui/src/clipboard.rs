//! System clipboard integration with two backends:
//!
//! - **native** ([`arboard`]) — talks to the OS clipboard. Confirms success,
//!   but over SSH it would target the *remote* machine's clipboard (or fail),
//!   which is useless to the person at the keyboard.
//! - **OSC 52** — an escape sequence the terminal itself interprets and copies
//!   to *your* clipboard. This is what makes copy work over SSH and through
//!   tmux. It is fire-and-forget: the terminal gives no acknowledgement, so a
//!   successful write means "sent to the terminal", not "the terminal copied".
//!
//! [`ClipboardMode::Auto`] prefers OSC 52 inside an SSH session and the native
//! backend otherwise (falling back to OSC 52 if the native backend errors).
//! The chosen [`Method`] is returned so the caller can report it — we never
//! switch backends silently.

use std::io::Write;

/// Which clipboard backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ClipboardMode {
    /// OSC 52 in an SSH session, native otherwise (with OSC 52 as a fallback).
    Auto,
    /// Always use the OS-native clipboard (`arboard`).
    Native,
    /// Always emit an OSC 52 terminal escape sequence (works over SSH / tmux).
    Osc52,
}

/// The backend that actually performed a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Native,
    Osc52,
}

impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Method::Native => "native",
            Method::Osc52 => "osc52",
        }
    }
}

/// Copy `text` to the clipboard using `mode`. Returns the [`Method`] used on
/// success, or a human-readable error.
pub fn copy(text: &str, mode: ClipboardMode) -> Result<Method, String> {
    match mode {
        ClipboardMode::Native => copy_native(text).map(|()| Method::Native),
        ClipboardMode::Osc52 => copy_osc52(text).map(|()| Method::Osc52),
        ClipboardMode::Auto => {
            if in_ssh() {
                copy_osc52(text).map(|()| Method::Osc52)
            } else {
                // Local: prefer the confirmable native backend; if it isn't
                // available (e.g. headless Linux), fall back to OSC 52 — the
                // returned Method tells the caller which path ran.
                match copy_native(text) {
                    Ok(()) => Ok(Method::Native),
                    Err(_) => copy_osc52(text).map(|()| Method::Osc52),
                }
            }
        }
    }
}

fn in_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

fn copy_native(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}

fn copy_osc52(text: &str) -> Result<(), String> {
    let seq = build_osc52(text, in_tmux());
    let mut out = std::io::stdout().lock();
    out.write_all(seq.as_bytes()).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

/// Build the OSC 52 "set clipboard" sequence for `text`. When `tmux` is true,
/// wrap it in tmux's DCS passthrough (doubling the inner ESCs) so it reaches
/// the outer terminal.
fn build_osc52(text: &str, tmux: bool) -> String {
    let core = format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()));
    if tmux {
        let escaped = core.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;{escaped}\x1b\\")
    } else {
        core
    }
}

/// Standard base64 (RFC 4648, with padding). Kept local to avoid a dependency.
fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn osc52_plain_sequence() {
        // ESC ] 52 ; c ; <b64> BEL
        assert_eq!(build_osc52("foo", false), "\x1b]52;c;Zm9v\x07");
    }

    #[test]
    fn osc52_tmux_passthrough_doubles_esc_and_wraps() {
        let seq = build_osc52("foo", true);
        assert!(seq.starts_with("\x1bPtmux;"));
        assert!(seq.ends_with("\x1b\\"));
        // The single inner ESC (start of the OSC) is doubled; the BEL is not.
        assert_eq!(seq, "\x1bPtmux;\x1b\x1b]52;c;Zm9v\x07\x1b\\");
    }
}
