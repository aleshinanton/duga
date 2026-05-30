//! Theme system with configurable color palette.
//!
//! Every widget references `theme.colors.xxx` instead of raw `Color::Rgb(...)`.
//! Themes are loaded from `TuiConfig` with `Theme::dark()` as the default.

use ratatui::style::{Color, Modifier, Style};

/// Named color roles used by all widgets.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeColors {
    pub bg: Color,
    pub surface: Color,
    pub primary: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub muted: Color,
    pub text: Color,
    pub text_dim: Color,
    pub border: Color,
    pub accent: Color,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self::dark()
    }
}

impl ThemeColors {
    /// Dark theme matching the redesign spec.
    pub fn dark() -> Self {
        Self {
            bg: Color::Rgb(10, 12, 16),
            surface: Color::Rgb(20, 20, 20),
            primary: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            muted: Color::DarkGray,
            text: Color::White,
            text_dim: Color::Gray,
            border: Color::Rgb(60, 60, 60),
            accent: Color::Cyan,
        }
    }

    /// Light theme variant.
    pub fn light() -> Self {
        Self {
            bg: Color::Rgb(245, 245, 245),
            surface: Color::Rgb(235, 235, 235),
            primary: Color::Rgb(0, 100, 200),
            success: Color::Rgb(0, 150, 0),
            warning: Color::Rgb(200, 150, 0),
            error: Color::Rgb(200, 0, 0),
            muted: Color::Rgb(180, 180, 180),
            text: Color::Rgb(30, 30, 30),
            text_dim: Color::Rgb(120, 120, 120),
            border: Color::Rgb(200, 200, 200),
            accent: Color::Rgb(0, 100, 200),
        }
    }
}

/// Full theme including colors and style constructors.
#[derive(Clone, Debug)]
pub struct Theme {
    pub colors: ThemeColors,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            colors: ThemeColors::dark(),
        }
    }
}

impl Theme {
    /// Create the dark theme (default).
    pub fn dark() -> Self {
        Self {
            colors: ThemeColors::dark(),
        }
    }

    /// Create the light theme.
    pub fn light() -> Self {
        Self {
            colors: ThemeColors::light(),
        }
    }

    /// Create a theme from config name. Falls back to dark on unknown names.
    pub fn from_config(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "dark" | "default" => Self::dark(),
            "light" => Self::light(),
            other => {
                tracing::warn!("Unknown theme '{other}', falling back to dark");
                Self::dark()
            }
        }
    }

    // ── Style constructors ──────────────────────────────────────────

    pub fn base_style(&self) -> Style {
        Style::default().bg(self.colors.bg)
    }

    pub fn surface_style(&self) -> Style {
        Style::default().bg(self.colors.surface)
    }

    pub fn text_style(&self) -> Style {
        Style::default().fg(self.colors.text)
    }

    pub fn text_dim_style(&self) -> Style {
        Style::default().fg(self.colors.text_dim)
    }

    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.colors.muted)
    }

    pub fn muted_italic_style(&self) -> Style {
        Style::default()
            .fg(self.colors.muted)
            .add_modifier(Modifier::ITALIC)
    }

    pub fn user_style(&self) -> Style {
        Style::default()
            .fg(self.colors.primary)
            .add_modifier(Modifier::BOLD)
    }

    pub fn assistant_style(&self) -> Style {
        Style::default()
            .fg(self.colors.success)
            .add_modifier(Modifier::BOLD)
    }

    pub fn error_style(&self) -> Style {
        Style::default()
            .fg(self.colors.error)
            .add_modifier(Modifier::BOLD)
    }

    pub fn warning_style(&self) -> Style {
        Style::default()
            .fg(self.colors.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn system_style(&self) -> Style {
        Style::default().fg(self.colors.accent)
    }

    pub fn status_style(&self, color: Color) -> Style {
        Style::default().fg(Color::Black).bg(color)
    }

    pub fn tool_running_style(&self) -> Style {
        Style::default()
            .fg(self.colors.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn tool_success_style(&self) -> Style {
        Style::default()
            .fg(self.colors.success)
            .add_modifier(Modifier::BOLD)
    }

    pub fn tool_error_style(&self) -> Style {
        Style::default()
            .fg(self.colors.error)
            .add_modifier(Modifier::BOLD)
    }

    pub fn delegation_style(&self) -> Style {
        Style::default().fg(Color::Magenta)
    }

    pub fn memory_style(&self) -> Style {
        Style::default().fg(self.colors.muted)
    }

    pub fn border_style(&self) -> Style {
        Style::default().fg(self.colors.border)
    }

    pub fn separator_style(&self) -> Style {
        Style::default().fg(self.colors.border)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_has_correct_colors() {
        let theme = Theme::dark();
        assert_eq!(theme.colors.bg, Color::Rgb(10, 12, 16));
        assert_eq!(theme.colors.primary, Color::Cyan);
        assert_eq!(theme.colors.success, Color::Green);
        assert_eq!(theme.colors.warning, Color::Yellow);
        assert_eq!(theme.colors.error, Color::Red);
    }

    #[test]
    fn from_config_dark() {
        let theme = Theme::from_config("dark");
        assert_eq!(theme.colors.bg, Color::Rgb(10, 12, 16));
    }

    #[test]
    fn from_config_light() {
        let theme = Theme::from_config("light");
        assert_eq!(theme.colors.bg, Color::Rgb(245, 245, 245));
    }

    #[test]
    fn from_config_unknown_falls_back_to_dark() {
        let theme = Theme::from_config("unicorn");
        assert_eq!(theme.colors.bg, Color::Rgb(10, 12, 16));
    }

    #[test]
    fn style_constructors_produce_correct_styles() {
        let theme = Theme::dark();
        let user = theme.user_style();
        assert_eq!(user.fg, Some(Color::Cyan));
        assert!(user.add_modifier.contains(Modifier::BOLD));

        let assistant = theme.assistant_style();
        assert_eq!(assistant.fg, Some(Color::Green));
        assert!(assistant.add_modifier.contains(Modifier::BOLD));

        let err = theme.error_style();
        assert_eq!(err.fg, Some(Color::Red));
        assert!(err.add_modifier.contains(Modifier::BOLD));

        let muted = theme.muted_style();
        assert_eq!(muted.fg, Some(Color::DarkGray));
    }

    #[test]
    fn default_theme_is_dark() {
        let theme = Theme::default();
        assert_eq!(theme.colors.bg, Color::Rgb(10, 12, 16));
    }
}
