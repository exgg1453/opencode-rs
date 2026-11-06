//! Multi-line prompt editor (pure logic) and key mapping.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use unicode_width::UnicodeWidthChar;

/// Multi-line text buffer. The cursor column is counted in chars.
#[derive(Debug, Clone, PartialEq)]
pub struct Editor {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
}

impl Default for Editor {
    fn default() -> Self {
        Editor { lines: vec![String::new()], row: 0, col: 0 }
    }
}

fn byte_idx(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
}

impl Editor {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }
    pub fn clear(&mut self) {
        *self = Editor::default();
    }
    pub fn set_text(&mut self, t: &str) {
        self.lines = t.split('\n').map(str::to_string).collect();
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
    }
    fn line_len(&self) -> usize {
        self.lines[self.row].chars().count()
    }
    pub fn insert_char(&mut self, c: char) {
        let i = byte_idx(&self.lines[self.row], self.col);
        self.lines[self.row].insert(i, c);
        self.col += 1;
    }
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                self.newline();
            } else if c != '\r' {
                self.insert_char(c);
            }
        }
    }
    pub fn newline(&mut self) {
        let i = byte_idx(&self.lines[self.row], self.col);
        let rest = self.lines[self.row].split_off(i);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }
    pub fn backspace(&mut self) {
        if self.col > 0 {
            let i = byte_idx(&self.lines[self.row], self.col - 1);
            self.lines[self.row].remove(i);
            self.col -= 1;
        } else if self.row > 0 {
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len();
            self.lines[self.row].push_str(&cur);
        }
    }
    pub fn delete(&mut self) {
        if self.col < self.line_len() {
            let i = byte_idx(&self.lines[self.row], self.col);
            self.lines[self.row].remove(i);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }
    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.line_len();
        }
    }
    pub fn right(&mut self) {
        if self.col < self.line_len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }
    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.line_len());
        } else {
            self.col = 0;
        }
    }
    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.line_len());
        } else {
            self.col = self.line_len();
        }
    }
    pub fn home(&mut self) {
        self.col = 0;
    }
    pub fn end(&mut self) {
        self.col = self.line_len();
    }
    /// Delete from line start to cursor.
    pub fn kill_to_start(&mut self) {
        let i = byte_idx(&self.lines[self.row], self.col);
        self.lines[self.row].drain(..i);
        self.col = 0;
    }
    /// Delete previous word.
    pub fn delete_word(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut c = self.col;
        while c > 0 && chars[c - 1].is_whitespace() {
            c -= 1;
        }
        while c > 0 && !chars[c - 1].is_whitespace() {
            c -= 1;
        }
        let (a, b) = (byte_idx(&self.lines[self.row], c), byte_idx(&self.lines[self.row], self.col));
        self.lines[self.row].drain(a..b);
        self.col = c;
    }

    /// Hard-wrap into visual rows of `width` columns; returns rows and the cursor (row, col).
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(2);
        let mut rows = Vec::new();
        let mut cursor = (0, 0);
        for (r, line) in self.lines.iter().enumerate() {
            let mut cur = String::new();
            let mut w = 0;
            let mut ci = 0;
            if r == self.row && self.col == 0 {
                cursor = (rows.len(), 0);
            }
            for ch in line.chars() {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                if w + cw > width {
                    rows.push(std::mem::take(&mut cur));
                    w = 0;
                }
                cur.push(ch);
                w += cw;
                ci += 1;
                if r == self.row && ci == self.col {
                    cursor = (rows.len(), w);
                }
            }
            rows.push(cur);
        }
        (rows, cursor)
    }
}

/// What a key press means at the app level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Submit,
    Newline,
    Quit,
    Interrupt,
    NewSession,
    SessionList,
    ScrollUp(usize),
    ScrollDown(usize),
    ToggleTools,
    Edit,
    None,
}

/// Map a key event to an action for the prompt (non-popup) mode. Editing keys return
/// `Action::Edit` after being applied to `ed`.
pub fn handle_key(ed: &mut Editor, key: KeyEvent) -> Action {
    if key.kind == KeyEventKind::Release {
        return Action::None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('n') if ctrl => Action::NewSession,
        KeyCode::Char('l') if ctrl => Action::SessionList,
        KeyCode::Char('o') if ctrl => Action::ToggleTools,
        KeyCode::Char('j') if ctrl => {
            ed.newline();
            Action::Edit
        }
        KeyCode::Char('u') if ctrl => {
            ed.kill_to_start();
            Action::Edit
        }
        KeyCode::Char('w') if ctrl => {
            ed.delete_word();
            Action::Edit
        }
        KeyCode::Char('a') if ctrl => {
            ed.home();
            Action::Edit
        }
        KeyCode::Char('e') if ctrl => {
            ed.end();
            Action::Edit
        }
        KeyCode::Enter if shift || alt => {
            ed.newline();
            Action::Edit
        }
        KeyCode::Enter => Action::Submit,
        KeyCode::Esc => Action::Interrupt,
        KeyCode::PageUp => Action::ScrollUp(10),
        KeyCode::PageDown => Action::ScrollDown(10),
        KeyCode::Backspace => {
            ed.backspace();
            Action::Edit
        }
        KeyCode::Delete => {
            ed.delete();
            Action::Edit
        }
        KeyCode::Left => {
            ed.left();
            Action::Edit
        }
        KeyCode::Right => {
            ed.right();
            Action::Edit
        }
        KeyCode::Up => {
            ed.up();
            Action::Edit
        }
        KeyCode::Down => {
            ed.down();
            Action::Edit
        }
        KeyCode::Home => {
            ed.home();
            Action::Edit
        }
        KeyCode::End => {
            ed.end();
            Action::Edit
        }
        KeyCode::Tab => {
            ed.insert_str("  ");
            Action::Edit
        }
        KeyCode::Char(c) if !ctrl => {
            ed.insert_char(c);
            Action::Edit
        }
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn typing_and_newlines() {
        let mut e = Editor::default();
        for c in "héllo".chars() {
            e.insert_char(c);
        }
        e.left();
        e.left();
        e.newline();
        assert_eq!(e.lines, vec!["hél", "lo"]);
        assert_eq!((e.row, e.col), (1, 0));
        e.backspace();
        assert_eq!(e.text(), "héllo");
        assert_eq!((e.row, e.col), (0, 3));
        e.delete();
        assert_eq!(e.text(), "hélo");
    }

    #[test]
    fn navigation_and_kill() {
        let mut e = Editor::default();
        e.insert_str("abc def\nxy");
        e.up();
        assert_eq!((e.row, e.col), (0, 2));
        e.end();
        e.delete_word();
        assert_eq!(e.lines[0], "abc ");
        e.kill_to_start();
        assert_eq!(e.lines[0], "");
        e.down();
        e.down();
        assert_eq!(e.col, 2);
    }

    #[test]
    fn key_mapping() {
        let mut e = Editor::default();
        assert_eq!(handle_key(&mut e, key(KeyCode::Char('a'), KeyModifiers::NONE)), Action::Edit);
        assert_eq!(handle_key(&mut e, key(KeyCode::Enter, KeyModifiers::NONE)), Action::Submit);
        assert_eq!(handle_key(&mut e, key(KeyCode::Enter, KeyModifiers::SHIFT)), Action::Edit);
        assert_eq!(handle_key(&mut e, key(KeyCode::Enter, KeyModifiers::ALT)), Action::Edit);
        assert_eq!(e.lines.len(), 3);
        assert_eq!(handle_key(&mut e, key(KeyCode::Char('c'), KeyModifiers::CONTROL)), Action::Quit);
        assert_eq!(handle_key(&mut e, key(KeyCode::Esc, KeyModifiers::NONE)), Action::Interrupt);
        assert_eq!(handle_key(&mut e, key(KeyCode::Char('l'), KeyModifiers::CONTROL)), Action::SessionList);
        assert_eq!(handle_key(&mut e, key(KeyCode::Char('n'), KeyModifiers::CONTROL)), Action::NewSession);
        assert_eq!(handle_key(&mut e, key(KeyCode::PageUp, KeyModifiers::NONE)), Action::ScrollUp(10));
    }

    #[test]
    fn layout_wraps_and_tracks_cursor() {
        let mut e = Editor::default();
        e.insert_str("abcdef\ngh");
        let (rows, cur) = e.layout(4);
        assert_eq!(rows, vec!["abcd", "ef", "gh"]);
        assert_eq!(cur, (2, 2));
        e.up();
        e.end();
        let (_, cur) = e.layout(4);
        assert_eq!(cur, (1, 2));
    }
}
