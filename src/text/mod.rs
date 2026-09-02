//! Text processing helpers: word wrapping and alignment.

pub mod figlet;

use crate::style::HAlign;
use unicode_width::UnicodeWidthStr;

/// Wrap `text` so that each line fits within `max_width` characters (measured
/// in display width, not raw `str::len`).
///
/// Tokens are broken at whitespace, common separator characters (`_`, `-`,
/// `.`, `/`), and CJK character boundaries. A single token longer than
/// `max_width` is broken mid-token. Blank input yields an empty vector.
pub fn word_wrap(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return text.lines().map(String::from).collect();
    }

    let mut lines: Vec<String> = Vec::new();
    for input_line in text.lines() {
        if input_line.is_empty() {
            lines.push(String::new());
            continue;
        }

        let tokens = tokenize(input_line);
        let mut current = String::new();
        let mut current_width = 0usize;

        for (token, needs_space) in tokens {
            if token.is_empty() {
                continue;
            }
            let token_width = UnicodeWidthStr::width(token.as_str());
            let space_width = if current.is_empty() || !needs_space { 0 } else { 1 };

            if current_width + space_width + token_width <= max_width {
                if needs_space && !current.is_empty() {
                    current.push(' ');
                    current_width += 1;
                }
                current.push_str(&token);
                current_width += token_width;
            } else {
                // Flush the current line if it has content.
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }

                // If the token itself exceeds max_width, break it.
                if token_width > max_width {
                    let mut remaining = token.clone();
                    while UnicodeWidthStr::width(&*remaining) > max_width {
                        let (chunk, rest) = split_at_display_width(&remaining, max_width);
                        let rest_w = UnicodeWidthStr::width(&*rest);
                        // Avoid leaving a single CJK character (width 2) on
                        // its own line: if the remainder is a lone CJK char,
                        // try to pull it back onto the current chunk — but
                        // only if the merged result doesn't exceed max_width.
                        if rest_w <= 2 && !chunk.is_empty() {
                            let merged = format!("{}{}", chunk, rest);
                            let merged_w = UnicodeWidthStr::width(&*merged);
                            if merged_w <= max_width {
                                lines.push(merged);
                                remaining.clear();
                                break;
                            }
                            // Merged would exceed max_width — fall through
                            // to push chunk alone and let rest go to next line.
                        }
                        lines.push(chunk);
                        remaining = rest;
                    }
                    if !remaining.is_empty() {
                        current.push_str(&remaining);
                        current_width = UnicodeWidthStr::width(&*current);
                    }
                } else {
                    current.push_str(&token);
                    current_width = token_width;
                }
            }
        }

        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}

/// Split text into tokens at whitespace, separator characters (`_`, `-`, `.`,
/// `/`), and CJK boundaries. Returns `(token, needs_space_before)` pairs.
/// Separators stay attached to the preceding token (e.g. `driver_bind` →
/// `driver_`, `bind`). Consecutive CJK characters are grouped into a single
/// token so wrapping doesn't isolate a single CJK character on its own line.
fn tokenize(text: &str) -> Vec<(String, bool)> {
    let mut tokens: Vec<(String, bool)> = Vec::new();
    let mut current = String::new();
    let mut after_space = false;

    let separators = ['_', '-', '.', '/'];

    for ch in text.chars() {
        if ch == ' ' {
            if !current.is_empty() {
                tokens.push((std::mem::take(&mut current), after_space));
            }
            after_space = true;
        } else if separators.contains(&ch) {
            current.push(ch);
            tokens.push((std::mem::take(&mut current), after_space));
            after_space = false;
        } else if is_cjk(ch) {
            // Group consecutive CJK characters into a single token so the
            // wrapper doesn't isolate a single CJK character on its own line.
            if !current.is_empty() && !current.chars().last().is_some_and(is_cjk) {
                tokens.push((std::mem::take(&mut current), after_space));
                after_space = false;
            }
            current.push(ch);
        } else {
            if !current.is_empty() && current.chars().last().is_some_and(is_cjk) {
                tokens.push((std::mem::take(&mut current), after_space));
                after_space = false;
            }
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push((current, after_space));
    }
    tokens
}

/// Check if a character is a CJK or CJK-adjacent character (wide display width).
fn is_cjk(ch: char) -> bool {
    use unicode_width::UnicodeWidthChar;
    UnicodeWidthChar::width(ch).is_some_and(|w| w >= 2)
}

/// Split `s` at `display_width` counting Unicode display width.
pub fn split_at_display_width(s: &str, width: usize) -> (String, String) {
    let mut w = 0usize;
    let mut byte_idx = 0usize;
    for (i, ch) in s.char_indices() {
        let chw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + chw > width {
            break;
        }
        w += chw;
        byte_idx = i + ch.len_utf8();
    }
    if byte_idx == 0 {
        // Even one char doesn't fit; force at least one.
        if let Some(ch) = s.chars().next() {
            byte_idx = ch.len_utf8();
        }
    }
    let left = s[..byte_idx].to_string();
    let right = s[byte_idx..].to_string();
    (left, right)
}

/// A label wrapped to a maximum display width, with the metadata layout
/// engines need to size canvases and gaps.
#[derive(Clone, Debug, Default)]
pub struct WrappedLabel {
    /// The wrapped lines.
    pub lines: Vec<String>,
    /// Number of lines.
    pub line_count: usize,
    /// Display width of the widest line.
    pub max_width: usize,
}

/// Aesthetic box-width budget for diagram node labels, in percent of
/// the canvas width (user-approved 2026-09-02). Flowchart nodes and
/// state boxes wrap their labels at this fraction of the user-chosen
/// width: a box wider than this pushes the rightmost node, the side
/// rail, and every long line far right, so long step descriptions wrap
/// inside a narrower, taller box instead. Deliberately a proportion of
/// the user's width, not a constant — the budget scales with the canvas.
pub const NODE_WRAP_WIDTH_PCT: usize = 40;

/// Wrap a label to `max_width` display columns.
///
/// This is the standard entry point for diagram label wrapping: it
/// enforces a minimum width of 2 (so CJK glyphs always fit) and returns
/// the metadata layout engines need to size canvases and gaps.
pub fn wrap_label(label: &str, max_width: usize) -> WrappedLabel {
    let mw = max_width.max(2);
    let lines = word_wrap(label, mw);
    let count = lines.len();
    let max_w = lines.iter().map(|l| UnicodeWidthStr::width(l.as_str())).max().unwrap_or(0);
    WrappedLabel { lines, line_count: count, max_width: max_w }
}

/// Align text horizontally within `width`.
///
/// `text` should already be wrapped to fit `width`.
pub fn align_horizontal(lines: &[String], width: usize, align: HAlign) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let lw = UnicodeWidthStr::width(line.as_str());
            let padding = width.saturating_sub(lw);
            match align {
                HAlign::Left => format!("{}{}", line, " ".repeat(padding)),
                HAlign::Right => format!("{}{}", " ".repeat(padding), line),
                HAlign::Center => {
                    let left = padding / 2;
                    let right = padding - left;
                    format!("{}{}{}", " ".repeat(left), line, " ".repeat(right))
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::HAlign;

    #[test]
    fn test_basic_wrapping() {
        let wrapped = word_wrap("hello world foo", 8);
        assert_eq!(wrapped, vec!["hello", "world", "foo"]);
    }

    #[test]
    fn test_long_word_breaking() {
        let wrapped = word_wrap("supercalifragilistic", 5);
        assert_eq!(wrapped, vec!["super", "calif", "ragil", "istic"]);
    }

    #[test]
    fn test_multiline_input() {
        let wrapped = word_wrap("line one\nline two", 20);
        assert_eq!(wrapped, vec!["line one", "line two"]);
    }

    #[test]
    fn test_center_align() {
        let lines = vec!["Hi".to_string()];
        let aligned = align_horizontal(&lines, 6, HAlign::Center);
        assert_eq!(aligned, vec!["  Hi  "]);
    }

    #[test]
    fn test_wrap_label_cjk_long() {
        let label = "开始执行驱动注册流程并等待总线回调";
        let w = wrap_label(label, 10);
        assert!(w.line_count > 1, "should wrap into multiple lines: {:?}", w.lines);
        for line in &w.lines {
            let lw = UnicodeWidthStr::width(line.as_str());
            assert!(lw <= 10, "line '{line}' width {lw} exceeds 10");
        }
        assert!(w.max_width > 0);
    }

    #[test]
    fn test_wrap_label_no_truncation() {
        let label = "开始执行驱动注册流程并等待总线回调返回结果";
        let reconstructed: String = wrap_label(label, 8).lines.join("");
        assert_eq!(reconstructed, label, "wrapping must not lose characters");
    }

    #[test]
    fn test_wrap_label_short_label() {
        let w = wrap_label("hello", 30);
        assert_eq!(w.lines, vec!["hello"]);
        assert_eq!(w.line_count, 1);
        assert_eq!(w.max_width, 5);
    }

    #[test]
    fn test_smart_split_at_separators() {
        // driver_bind完成 (16 wide) at width 10: should split at _ not mid-word
        let lines = wrap_label("driver_bind完成", 10).lines;
        let all = lines.join("");
        assert_eq!(all, "driver_bind完成", "no chars lost: {lines:?}");
        // Should NOT have "bi" and "nd" as separate fragments
        let joined = lines.join("|");
        assert!(!joined.contains("bi|nd"), "should not split mid-word 'bind': {joined:?}");
    }

    #[test]
    fn test_cjk_no_spaces_inserted() {
        // CJK characters joined without spaces
        let label = "开始执行";
        let reconstructed: String = wrap_label(label, 6).lines.join("");
        assert_eq!(
            reconstructed,
            label,
            "no spaces inserted between CJK: {:?}",
            wrap_label(label, 6).lines
        );
    }
}
