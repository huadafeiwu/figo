//! FIGlet text banner rendering.

use crate::error::{FigoError, Result};
use crate::text::figlet;
use crate::text::wrap_label;
use unicode_width::UnicodeWidthStr;

/// Draw a FIGlet-style text banner.
///
/// Lines the bundled FIGlet font can render (printable ASCII) become
/// banners. Any other line (e.g. CJK — the font has no glyphs for it)
/// falls back to plain text wrapped to the width budget, so characters
/// are never silently dropped.
pub fn draw_banner(text: &str, width: usize) -> Result<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() || lines.iter().all(|l| l.trim().is_empty()) {
        return Err(FigoError::MissingFields("text is empty".to_string()));
    }

    let mut out = String::new();
    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            out.push('\n');
        }
        if figlet::can_render(line) {
            // render_figlet's own output (rows plus trailing newline) is
            // the block; blank lines render as six empty rows, matching
            // the historical output.
            out.push_str(&figlet::render_figlet(line)?);
        } else {
            // Fallback: plain text, wrapped to the width budget. With no
            // budget (width 0) the line renders unwrapped.
            let avail = if width > 0 { width } else { line.width().max(1) };
            for l in wrap_label(line, avail).lines {
                out.push_str(&l);
                out.push('\n');
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_banner_still_figlet() {
        let out = draw_banner("HELLO", 60).unwrap();
        assert!(out.contains("_"), "FIGlet glyphs missing:\n{out}");
        assert!(out.contains("|"), "FIGlet glyphs missing:\n{out}");
    }

    #[test]
    fn test_cjk_banner_falls_back_to_text() {
        // Regression: non-ASCII characters used to be silently dropped
        // (the FIGlet font has no glyphs for them), rendering blank rows.
        let out = draw_banner("中文测试", 60).unwrap();
        for ch in "中文测试".chars() {
            assert!(out.contains(ch), "char '{ch}' lost:\n{out}");
        }
    }

    #[test]
    fn test_mixed_banner_keeps_every_char() {
        let out = draw_banner("A中B", 60).unwrap();
        for ch in "A中B".chars() {
            assert!(out.contains(ch), "char '{ch}' lost:\n{out}");
        }
    }

    #[test]
    fn test_fallback_wraps_to_width() {
        let long: String = "中".repeat(30);
        let out = draw_banner(&long, 20).unwrap();
        let widest = out.lines().map(|l| l.width()).max().unwrap_or(0);
        assert!(widest <= 20, "fallback line wider than budget: {widest}\n{out}");
        assert_eq!(out.matches('中').count(), 30, "chars lost:\n{out}");
    }
}
