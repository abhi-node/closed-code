use ratatui::style::palette::tailwind;
use ratatui::style::Color;

use crate::mode::Mode;

#[allow(dead_code)]
pub struct TuiTheme;

#[allow(dead_code)]
impl TuiTheme {
    // ── Base ──
    pub const BG: Color = tailwind::SLATE.c950;
    pub const FG: Color = tailwind::SLATE.c200;
    pub const FG_DIM: Color = tailwind::SLATE.c500;
    pub const FG_MUTED: Color = tailwind::SLATE.c600;

    // ── Borders ──
    pub const BORDER: Color = tailwind::SLATE.c700;
    pub const BORDER_FOCUS: Color = tailwind::BLUE.c400;
    pub const BORDER_DIM: Color = tailwind::SLATE.c800;

    // ── Accents ──
    pub const ACCENT: Color = tailwind::BLUE.c400;

    // ── Semantic ──
    pub const SUCCESS: Color = tailwind::EMERALD.c400;
    pub const WARNING: Color = tailwind::AMBER.c400;
    pub const ERROR: Color = tailwind::RED.c400;

    // ── Message Roles (Phase 9c) ──
    pub const USER: Color = tailwind::CYAN.c400;
    pub const USER_BG: Color = tailwind::CYAN.c950;
    pub const ASSISTANT: Color = tailwind::VIOLET.c400;
    pub const TOOL: Color = tailwind::AMBER.c400;
    pub const AGENT: Color = tailwind::TEAL.c400;

    // ── Diff (Phase 9d) ──
    pub const DIFF_ADD: Color = tailwind::EMERALD.c400;
    pub const DIFF_ADD_BG: Color = tailwind::EMERALD.c950;
    pub const DIFF_DEL: Color = tailwind::RED.c400;
    pub const DIFF_DEL_BG: Color = tailwind::RED.c950;
    pub const DIFF_HUNK: Color = tailwind::BLUE.c400;
    pub const DIFF_CONTEXT: Color = tailwind::SLATE.c500;

    // ── Markdown Rendering (Phase 9d) ──
    pub const MD_HEADING: Color = tailwind::BLUE.c300;
    pub const MD_CODE_FG: Color = tailwind::AMBER.c300;
    pub const MD_CODE_BLOCK_FG: Color = tailwind::SLATE.c300;
    pub const MD_CODE_BLOCK_BG: Color = tailwind::SLATE.c900;
    pub const MD_LINK: Color = tailwind::BLUE.c400;
    pub const MD_BLOCKQUOTE: Color = tailwind::SLATE.c400;
    pub const MD_LIST_MARKER: Color = tailwind::SLATE.c500;

    // ── Mode Colors ──
    pub const MODE_EXPLORE: Color = tailwind::BLUE.c400;
    pub const MODE_PLAN: Color = tailwind::VIOLET.c400;
    pub const MODE_GUIDED: Color = tailwind::AMBER.c400;
    pub const MODE_EXECUTE: Color = tailwind::EMERALD.c400;
    pub const MODE_AUTO: Color = tailwind::RED.c400;

    // ── Gauge ──
    pub const GAUGE_LOW: Color = tailwind::EMERALD.c400;
    pub const GAUGE_MED: Color = tailwind::AMBER.c400;
    pub const GAUGE_HIGH: Color = tailwind::RED.c400;

    // ── Command Picker (Phase 9b) ──
    pub const PICKER_HIGHLIGHT_BG: Color = tailwind::BLUE.c800;
    pub const PICKER_HIGHLIGHT_FG: Color = tailwind::SLATE.c100;
    pub const PICKER_MATCH: Color = tailwind::AMBER.c400;

    // ── Spinner Frames ──
    pub const SPINNER_FRAMES: &'static [&'static str] =
        &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
}

/// Return the theme color for a given Mode.
pub fn mode_color(mode: &Mode) -> Color {
    match mode {
        Mode::Explore => TuiTheme::MODE_EXPLORE,
        Mode::Plan => TuiTheme::MODE_PLAN,
        Mode::Guided => TuiTheme::MODE_GUIDED,
        Mode::Execute => TuiTheme::MODE_EXECUTE,
        Mode::Auto => TuiTheme::MODE_AUTO,
    }
}

/// Return the uppercase label for a given Mode (status bar badge).
pub fn mode_label(mode: &Mode) -> &'static str {
    match mode {
        Mode::Explore => "EXPLORE",
        Mode::Plan => "PLAN",
        Mode::Guided => "GUIDED",
        Mode::Execute => "EXECUTE",
        Mode::Auto => "AUTO",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_colors_are_distinct() {
        let colors = [
            mode_color(&Mode::Explore),
            mode_color(&Mode::Plan),
            mode_color(&Mode::Guided),
            mode_color(&Mode::Execute),
            mode_color(&Mode::Auto),
        ];
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn mode_labels_are_uppercase() {
        for mode in [
            Mode::Explore,
            Mode::Plan,
            Mode::Guided,
            Mode::Execute,
            Mode::Auto,
        ] {
            let label = mode_label(&mode);
            assert_eq!(label, label.to_uppercase());
        }
    }
}
