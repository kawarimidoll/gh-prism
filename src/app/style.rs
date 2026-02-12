use super::ThemeMode;
use ratatui::style::{Color, Modifier, Style};

/// PR Description のマークダウンレンダリング用カスタム StyleSheet
#[derive(Clone, Copy, Debug)]
pub(super) struct PrDescStyleSheet {
    pub(super) theme: ThemeMode,
}

impl tui_markdown::StyleSheet for PrDescStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match self.theme {
            ThemeMode::Dark => match level {
                1 => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                2 => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                3 => Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                _ => Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            },
            ThemeMode::Light => match level {
                1 => Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                2 => Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                3 => Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                _ => Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            },
        }
    }

    fn code(&self) -> Style {
        match self.theme {
            // 256色パレットのグレースケール（232=最暗, 255=最明）
            ThemeMode::Dark => Style::default().bg(Color::Indexed(238)),
            ThemeMode::Light => Style::default().bg(Color::Indexed(253)),
        }
    }

    fn link(&self) -> Style {
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        match self.theme {
            ThemeMode::Dark => Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
            ThemeMode::Light => Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        }
    }

    fn heading_meta(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    fn metadata_block(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }
}
