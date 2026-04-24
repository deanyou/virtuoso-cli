use ratatui::style::Color;

/// UI color palette. All color access goes through `Theme` so `no_color` mode
/// can fall back to `Modifier::REVERSED` for accessibility.
pub struct Theme {
    pub no_color: bool,
    pub primary: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    #[allow(dead_code)]
    pub warning: Color,
    pub text: Color,
    pub text_dim: Color,
    pub border: Color,
    #[allow(dead_code)]
    pub bg: Color,
    pub bg_selected: Color,
    pub surface: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            no_color: false,
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
            surface: Color::Rgb(60, 60, 72),
        }
    }
}

impl Theme {
    /// Detect no_color from the `NO_COLOR` environment variable per https://no-color.org.
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        Self {
            no_color,
            ..Self::default()
        }
    }

    #[allow(dead_code)]
    /// Force no_color on or off (from `--no-color` CLI flag).
    pub fn with_no_color(mut self, no_color: bool) -> Self {
        self.no_color = self.no_color || no_color;
        self
    }
}
