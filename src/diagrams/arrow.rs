//! Standalone arrows, lines, and connectors.
//!
//! Provides both a free function ([`draw_arrow`]) for simple usage and a
//! builder ([`Arrow`]) for complex configurations.

use std::fmt;

use crate::canvas::Canvas;
use crate::error::Result;
use crate::style::{Charset, LineStyle};
use unicode_width::UnicodeWidthStr;

/// Draw a standalone arrow.
///
/// Returns the rendered arrow as a `String`.
///
/// # Arguments
/// * `direction` — One of `"right"`, `"left"`, `"up"`, `"down"`, or `"bidirectional"`.
/// * `length` — The arrow length in characters (excluding arrowhead).
/// * `style` — Line style to use.
/// * `charset` — Character set mode.
/// * `label` — Optional label placed above the arrow.
pub fn draw_arrow(
    direction: &str,
    length: usize,
    style: LineStyle,
    charset: Charset,
    label: Option<&str>,
) -> Result<String> {
    let mut arrow = Arrow::new(direction, length, style, charset)?;
    if let Some(l) = label {
        arrow = arrow.label(l);
    }
    arrow.build()
}

/// Builder for drawing arrows.
pub struct Arrow {
    direction: String,
    length: usize,
    style: LineStyle,
    charset: Charset,
    label: Option<String>,
}

impl Arrow {
    /// Create a new arrow builder.
    pub fn new(direction: &str, length: usize, style: LineStyle, charset: Charset) -> Result<Self> {
        if !matches!(direction, "right" | "left" | "up" | "down" | "bidirectional") {
            return Err(crate::error::FigoError::InvalidDimensions(format!(
                "unknown direction '{direction}'"
            )));
        }
        Ok(Self { direction: direction.to_string(), length, style, charset, label: None })
    }

    /// Set an optional label for the arrow.
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_string());
        self
    }

    /// Render and return the arrow as a `String`.
    ///
    /// This is the primary rendering method. For `Display`-based output,
    /// format the builder directly.
    pub fn build(&self) -> Result<String> {
        match self.direction.as_str() {
            "right" | "left" | "bidirectional" => self.draw_horizontal(),
            "up" | "down" => self.draw_vertical(),
            _ => Err(crate::error::FigoError::General("unreachable".into())),
        }
    }

    fn arrow_chars(&self) -> (&'static str, &'static str) {
        match (self.style, self.charset) {
            (LineStyle::Simple, Charset::Unicode) => ("─", "→"),
            (LineStyle::Simple, Charset::Ascii) => ("-", ">"),
            (LineStyle::Bold, Charset::Unicode) => ("━", "⇒"),
            (LineStyle::Bold, Charset::Ascii) => ("=", ">"),
            (LineStyle::BoxDrawing, Charset::Unicode) => ("─", "→"),
            (LineStyle::BoxDrawing, Charset::Ascii) => ("-", ">"),
        }
    }

    fn left_arrow_chars(&self) -> (&'static str, &'static str) {
        match (self.style, self.charset) {
            (LineStyle::Simple, Charset::Unicode) => ("─", "←"),
            (LineStyle::Simple, Charset::Ascii) => ("-", "<"),
            (LineStyle::Bold, Charset::Unicode) => ("━", "⇐"),
            (LineStyle::Bold, Charset::Ascii) => ("=", "<"),
            (LineStyle::BoxDrawing, Charset::Unicode) => ("─", "←"),
            (LineStyle::BoxDrawing, Charset::Ascii) => ("-", "<"),
        }
    }

    fn draw_horizontal(&self) -> Result<String> {
        use crate::text::wrap_label;
        let (line, right_head) = self.arrow_chars();
        let (_, left_head) = self.left_arrow_chars();

        let label = self.label.as_deref().unwrap_or("");
        let left_head_width = left_head.width();
        let right_head_width = right_head.width();
        let total_width = if self.direction == "bidirectional" {
            self.length + left_head_width + right_head_width
        } else {
            self.length + right_head_width
        };

        // Wrap long labels instead of letting them grow the canvas unbounded.
        let max_label_w = total_width.max(60);
        let (label_lines, num_lines, max_line_w) = if label.is_empty() {
            (Vec::new(), 0usize, 0usize)
        } else {
            wrap_label(label, max_label_w)
        };
        let height = if label.is_empty() { 1usize } else { num_lines + 2 };
        let width = total_width.max(max_line_w);

        let mut canvas = Canvas::new(width, height);
        let arrow_y = if label.is_empty() { 0 } else { num_lines + 1 };

        match self.direction.as_str() {
            "right" => {
                let body: String = line.repeat(self.length);
                canvas.put_str(0, arrow_y, &body);
                canvas.put_str(self.length, arrow_y, right_head);
            }
            "left" => {
                canvas.put_str(0, arrow_y, left_head);
                let body: String = line.repeat(self.length);
                let head_width = left_head.width();
                canvas.put_str(head_width, arrow_y, &body);
            }
            "bidirectional" => {
                canvas.put_str(0, arrow_y, left_head);
                let body: String = line.repeat(self.length);
                let head_width = left_head.width();
                canvas.put_str(head_width, arrow_y, &body);
                canvas.put_str(head_width + self.length, arrow_y, right_head);
            }
            _ => unreachable!(),
        }

        if !label.is_empty() {
            for (i, l) in label_lines.iter().enumerate() {
                let lw = UnicodeWidthStr::width(l.as_str());
                let start_x = (total_width.saturating_sub(lw)) / 2;
                canvas.put_str(start_x, i, l);
            }
        }

        Ok(canvas.render(false))
    }

    fn draw_vertical(&self) -> Result<String> {
        use crate::text::wrap_label;
        let (head, line) = if self.direction == "up" {
            match (self.style, self.charset) {
                (LineStyle::Simple, Charset::Unicode) => ("↑", "│"),
                (LineStyle::Simple, Charset::Ascii) => ("^", "|"),
                (LineStyle::Bold, Charset::Unicode) => ("⇑", "┃"),
                (LineStyle::Bold, Charset::Ascii) => ("^", "|"),
                (LineStyle::BoxDrawing, Charset::Unicode) => ("↑", "│"),
                (LineStyle::BoxDrawing, Charset::Ascii) => ("^", "|"),
            }
        } else {
            match (self.style, self.charset) {
                (LineStyle::Simple, Charset::Unicode) => ("↓", "│"),
                (LineStyle::Simple, Charset::Ascii) => ("v", "|"),
                (LineStyle::Bold, Charset::Unicode) => ("⇓", "┃"),
                (LineStyle::Bold, Charset::Ascii) => ("v", "|"),
                (LineStyle::BoxDrawing, Charset::Unicode) => ("↓", "│"),
                (LineStyle::BoxDrawing, Charset::Ascii) => ("v", "|"),
            }
        };

        let label = self.label.as_deref().unwrap_or("");
        let label_len = label.width();
        // +1 so a label placed at x=1 (right of the vertical line at x=0)
        // doesn't have its last character clipped at the canvas edge.
        let max_label_w = 60usize;
        let (label_lines, _num_lines, max_line_w) = if label.is_empty() {
            (Vec::new(), 0usize, 0usize)
        } else {
            wrap_label(label, max_label_w)
        };
        let width = if label.is_empty() { 1usize } else { (label_len + 1).max(max_line_w + 1) };
        let height = self.length + 1;

        let mut canvas = Canvas::new(width, height);
        if self.direction == "up" {
            canvas.put_str(0, 0, head);
            for i in 0..self.length {
                canvas.put_str(0, i + 1, line);
            }
        } else {
            for i in 0..self.length {
                canvas.put_str(0, i, line);
            }
            canvas.put_str(0, self.length, head);
        }

        // Label on the right side of the vertical line, wrapped.
        if !label.is_empty() {
            for (i, l) in label_lines.iter().enumerate() {
                canvas.put_str(1, i, l);
            }
        }

        Ok(canvas.render(false))
    }
    /// Render and return as a `String`.
    ///
    /// Alias for [`build`](Self::build).
    pub fn render(&self) -> Result<String> {
        self.build()
    }
}

impl fmt::Display for Arrow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.build() {
            Ok(s) => write!(f, "{s}"),
            Err(e) => write!(f, "[figo error: {e}]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_right_arrow_ascii() {
        let out = draw_arrow("right", 5, LineStyle::Simple, Charset::Ascii, None).unwrap();
        assert_eq!(out.trim(), "----->");
    }

    #[test]
    fn test_left_arrow_unicode() {
        let out = draw_arrow("left", 3, LineStyle::Simple, Charset::Unicode, None).unwrap();
        assert_eq!(out.trim(), "←───");
    }

    #[test]
    fn test_with_label() {
        let out = draw_arrow("right", 5, LineStyle::Simple, Charset::Ascii, Some("flow")).unwrap();
        assert!(out.contains("flow"));
        assert!(out.contains("----->"));
    }

    #[test]
    fn test_bidirectional() {
        let out = draw_arrow("bidirectional", 3, LineStyle::Simple, Charset::Ascii, None).unwrap();
        assert_eq!(out.trim(), "<--->");
    }

    #[test]
    fn test_invalid_direction() {
        assert!(draw_arrow("north", 5, LineStyle::Simple, Charset::Ascii, None).is_err());
    }

    #[test]
    fn test_vertical_arrow_label_not_clipped() {
        let out = draw_arrow("down", 3, LineStyle::Simple, Charset::Ascii, Some("X")).unwrap();
        assert!(out.contains("X"), "single-char vertical label must be visible:\n{out}");
    }

    #[test]
    fn test_long_horizontal_label_wraps() {
        let long_label = "数据从用户空间传输到硬件设备的完整路径";
        let out =
            draw_arrow("right", 10, LineStyle::Simple, Charset::Ascii, Some(long_label)).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // Label should wrap into multiple lines, arrow on the last non-empty line.
        assert!(lines.len() > 2, "expected multi-line output:\n{out}");
        assert!(out.contains(">"), "arrowhead missing:\n{out}");
        // Every label character must appear somewhere in the output.
        for ch in long_label.chars() {
            assert!(out.contains(ch), "label char '{ch}' missing:\n{out}");
        }
    }
}
