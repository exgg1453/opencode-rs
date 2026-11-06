//! Color theme (loosely based on OpenCode's default dark theme).

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone)]
pub struct Theme {
    pub text: Style,
    pub muted: Style,
    pub accent: Style,
    pub user: Style,
    pub heading: Style,
    pub bold: Style,
    pub code_inline: Style,
    pub code_block: Style,
    pub tool: Style,
    pub tool_name: Style,
    pub error: Style,
    pub success: Style,
    pub status_bar: Style,
    pub border: Style,
    pub border_busy: Style,
    pub selected: Style,
}

impl Default for Theme {
    fn default() -> Self {
        let fg = Color::Rgb(0xee, 0xee, 0xee);
        let muted = Color::Rgb(0x80, 0x80, 0x80);
        let accent = Color::Rgb(0xfa, 0xb2, 0x83);
        let blue = Color::Rgb(0x5c, 0x9c, 0xf5);
        let red = Color::Rgb(0xe0, 0x6c, 0x75);
        let green = Color::Rgb(0x7f, 0xd8, 0x8f);
        Theme {
            text: Style::default().fg(fg),
            muted: Style::default().fg(muted),
            accent: Style::default().fg(accent),
            user: Style::default().fg(blue).add_modifier(Modifier::BOLD),
            heading: Style::default().fg(accent).add_modifier(Modifier::BOLD),
            bold: Style::default().fg(fg).add_modifier(Modifier::BOLD),
            code_inline: Style::default().fg(green),
            code_block: Style::default().fg(Color::Rgb(0xc8, 0xd3, 0xc0)),
            tool: Style::default().fg(muted),
            tool_name: Style::default().fg(blue),
            error: Style::default().fg(red),
            success: Style::default().fg(green),
            status_bar: Style::default().fg(fg).bg(Color::Rgb(0x1e, 0x1e, 0x1e)),
            border: Style::default().fg(Color::Rgb(0x48, 0x48, 0x48)),
            border_busy: Style::default().fg(accent),
            selected: Style::default().fg(Color::Black).bg(accent),
        }
    }
}
