//! CLI command handlers: JSON parsing and diagram dispatch.
//!
//! Each diagram type has a JSON input struct and a `run_*` function that
//! deserializes the input and calls the appropriate library function.
//!
//! Sub-modules are organized by diagram category to keep files under 250 lines.

mod flowchart;
mod network;
mod shapes;
mod state;
mod timeline;

pub use flowchart::run_flowchart;
pub use network::{run_arrow, run_packet, run_tree};
pub use shapes::{run_box, run_table};
pub use state::run_state;
pub use timeline::{run_banner, run_gantt, run_sequence};

use figo::style::{BorderStyle, Charset, HAlign};
use serde::Deserialize;

/// JSON character set — always "ascii" or "unicode".
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JsonCharset {
    Ascii,
    Unicode,
}

impl From<JsonCharset> for Charset {
    fn from(c: JsonCharset) -> Self {
        match c {
            JsonCharset::Ascii => Charset::Ascii,
            JsonCharset::Unicode => Charset::Unicode,
        }
    }
}

/// JSON border style.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JsonBorder {
    Single,
    Double,
    Rounded,
    Dashed,
    Bold,
}

pub(crate) fn border_from_json(b: JsonBorder) -> BorderStyle {
    match b {
        JsonBorder::Single => BorderStyle::Single,
        JsonBorder::Double => BorderStyle::Double,
        JsonBorder::Rounded => BorderStyle::Rounded,
        JsonBorder::Dashed => BorderStyle::Dashed,
        JsonBorder::Bold => BorderStyle::Bold,
    }
}

/// JSON padding specification.
#[derive(Deserialize)]
pub(crate) struct JsonPadding {
    pub horizontal: usize,
    pub vertical: usize,
}

/// JSON alignment specification.
#[derive(Deserialize)]
pub(crate) struct JsonAlignment {
    pub horizontal: String,
    pub vertical: String,
}

/// JSON position (x, y).
#[derive(Deserialize)]
pub(crate) struct JsonPosition {
    pub x: usize,
    pub y: usize,
}

/// Parse a JSON horizontal alignment string.
pub(crate) fn parse_halign(s: &str) -> HAlign {
    match s {
        "center" => HAlign::Center,
        "right" => HAlign::Right,
        _ => HAlign::Left,
    }
}

/// Parse a JSON vertical alignment string.
pub(crate) fn parse_valign(s: &str) -> figo::style::VAlign {
    match s {
        "middle" => figo::style::VAlign::Middle,
        "bottom" => figo::style::VAlign::Bottom,
        _ => figo::style::VAlign::Top,
    }
}

/// The raw detected terminal width, without the display margin.
/// Label-widening budgets use this (`max(canvas, terminal_width())`):
/// labels may drive geometry up to what the display can actually show,
/// even when the canvas itself is the margined default — discounting
/// the budget would re-wrap labels of diagrams that set an explicit
/// `width`.
pub fn terminal_width() -> usize {
    #[cfg(windows)]
    {
        detect_width_windows().unwrap_or(120)
    }
    #[cfg(not(windows))]
    {
        120
    }
}

/// Detect the default canvas width: `terminal_width()` scaled to
/// `DEFAULT_WIDTH_PCT` (the display margin). Falls back to 96 columns
/// (80% of 120) on non-Windows or failure.
///
/// Formula: columns = (screen_width_px / dpi_scale) / 16, clamped to
/// [80, 200], then discounted to 80%.
/// Examples: 1920px@96dpi → 120 → 96, 2560px@96dpi → 160 → 128,
/// 3840px@144dpi → 160 → 128.
pub fn default_width() -> usize {
    apply_display_margin(terminal_width())
}

#[cfg(windows)]
fn detect_width_windows() -> Option<usize> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGPIXELSX, ReleaseDC};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN};

    unsafe {
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        if screen_w == 0 {
            return None;
        }
        // Get DPI from the screen DC.
        let hwnd: HWND = std::ptr::null_mut();
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            // Fallback: assume 96 DPI (100%).
            let cols = (screen_w as f64 / 96.0 * 6.0).round() as usize;
            return Some(cols.clamp(80, 200));
        }
        let dpi = GetDeviceCaps(hdc, LOGPIXELSX as i32) as f64;
        let _ = ReleaseDC(hwnd, hdc);
        if dpi <= 0.0 {
            return None;
        }
        // Each character ≈ 16 pixels at 96 DPI; scale by DPI.
        let effective_w = screen_w as f64 / dpi * 96.0;
        let cols = (effective_w / 16.0).round() as usize;
        Some(cols.clamp(80, 200))
    }
}

/// Resolve width: use the user-specified value if non-zero, otherwise
/// detect from the display.
pub fn resolve_width(user_width: usize) -> usize {
    if user_width > 0 { user_width } else { default_width() }
}

/// Fraction of the detected terminal width used as the default canvas,
/// in percent. Leaves a display margin — prompt, line numbers, editor
/// side panels — so a resolution-derived diagram keeps about a fifth of
/// the terminal free and never wraps when pasted (user-approved
/// 2026-09-03). A JSON `width` always wins and is not discounted.
const DEFAULT_WIDTH_PCT: usize = 80;

/// Apply the display margin to a detected terminal width.
fn apply_display_margin(cols: usize) -> usize {
    cols * DEFAULT_WIDTH_PCT / 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_width_applies_display_margin() {
        // The default canvas is 80% of the detected terminal width;
        // explicit JSON `width` bypasses this entirely.
        assert_eq!(apply_display_margin(120), 96);
        assert_eq!(apply_display_margin(160), 128);
        assert_eq!(apply_display_margin(80), 64);
        assert_eq!(apply_display_margin(200), 160);
    }
}
