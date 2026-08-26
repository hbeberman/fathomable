// @okf-doc: /decisions/0010-viewer-ux.md
//! Clipboard access through the OSC 52 terminal escape.

use std::io::{self, Write};

/// Copy `text` to the terminal's clipboard with OSC 52.
///
/// No fallback: terminals without OSC 52 ignore the sequence (ADR 0010).
pub fn copy(text: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

/// Standard base64 with padding; small enough not to warrant a dependency.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut buf = [0u8; 3];
        buf[..chunk.len()].copy_from_slice(chunk);
        let n = u32::from_be_bytes([0, buf[0], buf[1], buf[2]]);
        let sextets = [n >> 18, n >> 12, n >> 6, n];
        for (i, sextet) in sextets.iter().enumerate() {
            let ch = if i <= chunk.len() {
                TABLE[(sextet & 0x3f) as usize] as char
            } else {
                '='
            };
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn encodes_with_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("héllo".as_bytes()), "aMOpbGxv");
    }
}
