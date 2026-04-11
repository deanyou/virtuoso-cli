use ratatui::style::Color;

pub struct Theme {
    pub primary: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub text: Color,
    pub text_dim: Color,
    pub border: Color,
    pub bg: Color,
    pub bg_selected: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            primary: Color::Rgb(0, 184, 212),
            accent: Color::Rgb(255, 193, 7),
            success: Color::Rgb(76, 175, 80),
            error: Color::Rgb(244, 67, 54),
            warning: Color::Rgb(255, 152, 0),
            text: Color::Rgb(224, 224, 224),
            text_dim: Color::Rgb(120, 120, 120),
            border: Color::Rgb(97, 97, 97),
            bg: Color::Rgb(33, 33, 33),
            bg_selected: Color::Rgb(42, 42, 55),
        }
    }
}
