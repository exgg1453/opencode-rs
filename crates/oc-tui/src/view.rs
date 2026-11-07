//! Rendering: transcript, prompt, status bar and popups.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as UiBlock, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::perm::draw_dialog;

use crate::markdown::{markdown_lines, wrap_text};
use crate::state::{AppState, Block};
use crate::theme::Theme;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const COLLAPSED_LINES: usize = 3;

/// Build all transcript lines for the given width.
pub fn transcript_lines(state: &AppState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let w = width.max(4);
    let mut out: Vec<Line<'static>> = Vec::new();
    for b in &state.blocks {
        match b {
            Block::User(t) => {
                for (i, l) in wrap_text(t, w.saturating_sub(2)).into_iter().enumerate() {
                    let prefix = if i == 0 { "> " } else { "  " };
                    out.push(Line::from(vec![Span::styled(prefix, theme.user), Span::styled(l, theme.text)]));
                }
            }
            Block::Assistant(t) => out.extend(markdown_lines(t, w, theme)),
            Block::Thinking(t) => {
                let style = theme.muted.add_modifier(Modifier::ITALIC);
                if state.expand_thinking {
                    out.push(Line::styled("▾ Thinking", style));
                    for l in t.trim_end().lines().flat_map(|l| wrap_text(l, w.saturating_sub(2))) {
                        out.push(Line::styled(format!("  {l}"), style));
                    }
                } else {
                    let first = t.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                    let line = format!("▸ Thinking: {first}");
                    out.push(Line::styled(format!("{}…", crate::state::truncate(&line, w.saturating_sub(1).max(1))), style));
                }
            }
            Block::Tool { name, summary, output, is_error, .. } => {
                let (icon, istyle) = match (output, is_error) {
                    (None, _) => ("…", theme.muted),
                    (Some(_), true) => ("✗", theme.error),
                    (Some(_), false) => ("✓", theme.success),
                };
                let mut spans = vec![
                    Span::styled(format!("{icon} "), istyle),
                    Span::styled(name.clone(), theme.tool_name.add_modifier(Modifier::BOLD)),
                ];
                if !summary.is_empty() {
                    let room = w.saturating_sub(name.chars().count() + 4);
                    spans.push(Span::styled(format!(" {}", crate::state::truncate(summary, room.max(1))), theme.tool));
                }
                out.push(Line::from(spans));
                if let Some(o) = output {
                    let style = if *is_error { theme.error } else { theme.tool };
                    let all: Vec<String> = o.lines().flat_map(|l| wrap_text(l, w.saturating_sub(4))).collect();
                    let limit = if state.expand_tools { all.len() } else { COLLAPSED_LINES };
                    for l in all.iter().take(limit) {
                        out.push(Line::styled(format!("  │ {l}"), style));
                    }
                    if all.len() > limit {
                        out.push(Line::styled(
                            format!("  │ … {} more lines (Ctrl+O to expand)", all.len() - limit),
                            theme.muted,
                        ));
                    }
                }
            }
            Block::Error(t) => {
                for l in wrap_text(&format!("Error: {t}"), w) {
                    out.push(Line::styled(l, theme.error));
                }
            }
            Block::Info(t) => {
                for l in wrap_text(t, w) {
                    out.push(Line::styled(l, theme.muted));
                }
            }
        }
        out.push(Line::default());
    }
    out
}

pub fn draw(f: &mut Frame, state: &mut AppState, theme: &Theme) {
    let area = f.area();
    f.render_widget(UiBlock::default().style(theme.base), area);
    let inner_w = area.width.saturating_sub(2) as usize;
    let (rows, cursor) = state.editor.layout(inner_w.saturating_sub(2).max(2));
    let prompt_h = (rows.len().min(8) as u16) + 2;
    let [main, prompt, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(prompt_h), Constraint::Length(1)]).areas(area);

    // Transcript
    let t_area = Rect { x: main.x + 1, width: main.width.saturating_sub(2), ..main };
    let lines = transcript_lines(state, t_area.width as usize, theme);
    let h = t_area.height as usize;
    let max_scroll = lines.len().saturating_sub(h);
    state.scroll = state.scroll.min(max_scroll);
    let top = max_scroll - state.scroll;
    if lines.is_empty() {
        let hint = Paragraph::new(vec![
            Line::default(),
            Line::styled("OpenCode (Rust)", theme.accent.add_modifier(Modifier::BOLD)),
            Line::styled("Type a message and press Enter. /help for commands.", theme.muted),
        ]);
        f.render_widget(hint, t_area);
    } else {
        f.render_widget(Paragraph::new(lines).scroll((top as u16, 0)), t_area);
    }

    // Prompt
    let bstyle = if state.busy { theme.border_busy } else { theme.border };
    let title = if state.busy { " working… (Esc to interrupt) " } else { " prompt " };
    let blk = UiBlock::default().borders(Borders::ALL).border_style(bstyle).title(Span::styled(title, theme.muted));
    let p_inner = blk.inner(prompt);
    let visible = p_inner.height as usize;
    let skip = (cursor.0 + 1).saturating_sub(visible);
    let shown: Vec<Line> = rows
        .iter()
        .skip(skip)
        .take(visible)
        .map(|r| Line::from(vec![Span::styled("┃ ", theme.accent), Span::styled(r.clone(), theme.text)]))
        .collect();
    f.render_widget(Paragraph::new(shown).block(blk), prompt);
    if state.popup.is_none() && state.theme_popup.is_none() && state.perm.is_empty() {
        f.set_cursor_position((p_inner.x + 2 + cursor.1 as u16, p_inner.y + (cursor.0 - skip) as u16));
    }

    // Status bar
    let spin = if state.busy { SPINNER[state.tick % SPINNER.len()] } else { " " };
    let think = state.thinking.map_or("off", |l| l.as_str());
    let left = format!(" {spin} {} │ thinking: {think} │ {} │ {}", state.model, state.cwd, state.title);
    let right = "Ctrl+T thinking  Ctrl+L sessions  Ctrl+C quit ";
    let pad = (status.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let text = format!("{left}{}{right}", " ".repeat(pad));
    f.render_widget(Paragraph::new(text).style(theme.status_bar), status);

    // Session popup
    if let Some(p) = &state.popup {
        let pw = area.width.saturating_sub(8).clamp(20, 70);
        let ph = (p.items.len() as u16 + 2).clamp(3, area.height.saturating_sub(4).max(3));
        let r = Rect {
            x: area.x + (area.width.saturating_sub(pw)) / 2,
            y: area.y + (area.height.saturating_sub(ph)) / 2,
            width: pw.min(area.width),
            height: ph.min(area.height),
        };
        f.render_widget(Clear, r);
        let blk = UiBlock::default()
            .borders(Borders::ALL)
            .border_style(theme.accent)
            .style(theme.panel)
            .title(" Sessions (Enter select, Esc close) ");
        let inner = blk.inner(r);
        let vis = inner.height as usize;
        let start = (p.selected + 1).saturating_sub(vis);
        let items: Vec<Line> = if p.items.is_empty() {
            vec![Line::styled("No sessions", theme.muted)]
        } else {
            p.items
                .iter()
                .enumerate()
                .skip(start)
                .take(vis)
                .map(|(i, s)| {
                    let t = crate::state::truncate(&s.title, inner.width.saturating_sub(2) as usize);
                    if i == p.selected {
                        Line::styled(format!(" {t}"), theme.selected)
                    } else {
                        Line::styled(format!(" {t}"), theme.text)
                    }
                })
                .collect()
        };
        f.render_widget(Paragraph::new(items).block(blk), r);
    }

    // Theme picker
    if let Some(p) = &state.theme_popup {
        let pw = 40u16.min(area.width);
        let ph = ((p.names.len() as u16) + 2).min(area.height.saturating_sub(2).max(3)).min(area.height);
        let r = Rect { x: area.x + area.width.saturating_sub(pw) / 2, y: area.y + area.height.saturating_sub(ph) / 2, width: pw, height: ph };
        f.render_widget(Clear, r);
        let blk = UiBlock::default()
            .borders(Borders::ALL)
            .border_style(theme.accent)
            .style(theme.panel)
            .title(" Themes (Enter select, Esc cancel) ");
        let vis = blk.inner(r).height as usize;
        let start = (p.selected + 1).saturating_sub(vis);
        let items: Vec<Line> = p
            .names
            .iter()
            .enumerate()
            .skip(start)
            .take(vis)
            .map(|(i, n)| {
                let st = if i == p.selected { theme.selected } else { theme.text };
                Line::styled(format!(" {n}"), st)
            })
            .collect();
        f.render_widget(Paragraph::new(items).block(blk), r);
    }

    // Permission dialog (topmost)
    draw_dialog(f, area, &state.perm, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn dump(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    fn state() -> AppState {
        AppState::new("anthropic/test-model".into(), "/work/dir".into(), "My Session".into())
    }

    #[test]
    fn renders_transcript_prompt_status() {
        let theme = Theme::default();
        let mut st = state();
        st.blocks.push(Block::User("hello there".into()));
        st.blocks.push(Block::Assistant("# Title\nsome **bold** and `code`".into()));
        st.blocks.push(Block::Tool {
            id: "1".into(),
            name: "bash".into(),
            summary: "ls".into(),
            output: Some("a\nb\nc\nd\ne".into()),
            is_error: false,
        });
        st.blocks.push(Block::Error("bad thing".into()));
        st.editor.insert_str("typed text");
        st.busy = true;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut st, &theme)).unwrap();
        let out = dump(&term);
        for needle in [
            "> hello there", "Title", "some bold and code", "✓ bash ls", "│ a", "2 more lines",
            "Error: bad thing", "typed text", "anthropic/test-model", "/work/dir", "My Session", "working",
        ] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
    }

    #[test]
    fn thinking_collapsed_and_expanded() {
        let theme = Theme::default();
        let mut st = state();
        st.thinking = Some(oc_ai::ThinkingLevel::Medium);
        st.blocks.push(Block::Thinking("first idea\nsecond idea".into()));
        st.blocks.push(Block::Assistant("the answer".into()));
        let lines = |st: &AppState| -> Vec<String> {
            transcript_lines(st, 60, &theme).iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect()
        };
        let l = lines(&st);
        assert_eq!(l[0], "▸ Thinking: first idea…");
        assert!(!l.iter().any(|x| x.contains("second idea")));
        assert!(l.iter().any(|x| x.contains("the answer")));
        st.toggle_thinking();
        let l = lines(&st);
        assert_eq!(l[0], "▾ Thinking");
        assert_eq!(l[1], "  first idea");
        assert_eq!(l[2], "  second idea");
        // muted theme role, no hard-coded colour
        let t = transcript_lines(&st, 60, &theme);
        assert_eq!(t[1].style.fg, theme.muted.fg);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut st, &theme)).unwrap();
        assert!(dump(&term).contains("thinking: medium"));
    }

    #[test]
    fn expanded_tools_show_all_and_scroll_clamps() {
        let theme = Theme::default();
        let mut st = state();
        st.expand_tools = true;
        st.blocks.push(Block::Tool {
            id: "1".into(),
            name: "read".into(),
            summary: String::new(),
            output: Some((0..100).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n")),
            is_error: false,
        });
        st.scroll = 10_000;
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        term.draw(|f| draw(f, &mut st, &theme)).unwrap();
        assert!(st.scroll < 10_000);
        assert!(dump(&term).contains("line0"));
    }

    #[test]
    fn renders_session_popup() {
        let theme = Theme::default();
        let mut st = state();
        let mut a = oc_core::Session::new("/".into());
        a.title = "First chat".into();
        st.popup = Some(crate::state::SessionPopup { items: vec![a], selected: 0 });
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut st, &theme)).unwrap();
        let out = dump(&term);
        assert!(out.contains("First chat") && out.contains("Sessions"));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let theme = Theme::default();
        let mut st = state();
        st.blocks.push(Block::Assistant("x".repeat(500)));
        let mut term = Terminal::new(TestBackend::new(10, 5)).unwrap();
        term.draw(|f| draw(f, &mut st, &theme)).unwrap();
    }
}
