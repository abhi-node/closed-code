use crossterm::style::Color;

pub struct Theme;

impl Theme {
    pub const USER: Color = Color::Cyan;
    pub const ASSISTANT: Color = Color::White;
    pub const ERROR: Color = Color::Red;
    pub const SUCCESS: Color = Color::Green;
    pub const DIM: Color = Color::DarkGrey;
    pub const ACCENT: Color = Color::Yellow;
    pub const PROMPT: Color = Color::Blue;

    // Phase 4: diff display colors
    pub const DIFF_ADD: Color = Color::Green;
    pub const DIFF_DELETE: Color = Color::Red;
    pub const DIFF_HUNK: Color = Color::Cyan;
    pub const DIFF_CONTEXT: Color = Color::DarkGrey;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_colors_are_distinct() {
        let colors = [
            Theme::USER,
            Theme::ERROR,
            Theme::SUCCESS,
            Theme::DIM,
            Theme::ACCENT,
            Theme::PROMPT,
        ];
        // Verify each color is unique
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "Colors at index {} and {} should differ", i, j);
                }
            }
        }
    }
}
