//! Pure text helpers: word wrapping and basic markdown to styled lines.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

/// Plain word-wrap of a string (honours embedded newlines). Never returns an empty vec.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let segs = vec![(raw.to_string(), Style::default())];
        for l in wrap_spans(&segs, width) {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            out.push(joined.trim_end().to_string());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Greedy word wrap of styled segments into lines no wider than `width`.
pub fn wrap_spans(segs: &[(String, Style)], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut cur = 0usize;

    fn push(lines: &mut [Vec<Span<'static>>], text: String, style: Style) {
        let l = lines.last_mut().unwrap();
        l.push(Span::styled(text, style));
    }

    for (text, style) in segs {
        // Tokenize into runs of whitespace / non-whitespace.
        let mut tokens: Vec<(String, bool)> = Vec::new();
        for ch in text.chars() {
            let sp = ch.is_whitespace();
            match tokens.last_mut() {
                Some((t, s)) if *s == sp => t.push(ch),
                _ => tokens.push((ch.to_string(), sp)),
            }
        }
        for (tok, is_space) in tokens {
            let w = UnicodeWidthStr::width(tok.as_str());
            if is_space {
                if cur == 0 {
                    // keep indentation on the very first line only
                    if lines.len() == 1 && w < width {
                        push(&mut lines, tok, *style);
                        cur += w;
                    }
                } else if cur + w <= width {
                    push(&mut lines, tok, *style);
                    cur += w;
                } else {
                    lines.push(Vec::new());
                    cur = 0;
                }
                continue;
            }
            if cur + w <= width {
                push(&mut lines, tok, *style);
                cur += w;
            } else if w <= width {
                lines.push(Vec::new());
                push(&mut lines, tok, *style);
                cur = w;
            } else {
                // Break an over-long word by characters.
                let mut buf = String::new();
                let mut bw = 0;
                for ch in tok.chars() {
                    let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if cur + bw + cw > width {
                        if !buf.is_empty() {
                            push(&mut lines, std::mem::take(&mut buf), *style);
                        }
                        lines.push(Vec::new());
                        cur = 0;
                        bw = 0;
                    }
                    buf.push(ch);
                    bw += cw;
                }
                if !buf.is_empty() {
                    push(&mut lines, buf, *style);
                }
                cur += bw;
            }
        }
    }
    lines.into_iter().map(Line::from).collect()
}

/// Parse inline markdown (`**bold**`, `` `code` ``) into styled segments.
pub fn inline_segments(text: &str, base: Style, theme: &Theme) -> Vec<(String, Style)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    let flush = |buf: &mut String, out: &mut Vec<(String, Style)>| {
        if !buf.is_empty() {
            out.push((std::mem::take(buf), base));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '`') {
                flush(&mut buf, &mut out);
                let code: String = chars[i + 1..i + 1 + end].iter().collect();
                out.push((code, theme.code_inline));
                i += end + 2;
                continue;
            }
        } else if c == '*' && chars.get(i + 1) == Some(&'*') {
            let rest = &chars[i + 2..];
            if let Some(end) = rest.windows(2).position(|w| w == ['*', '*']) {
                if end > 0 {
                    flush(&mut buf, &mut out);
                    let inner: String = rest[..end].iter().collect();
                    out.push((inner, theme.bold));
                    i += end + 4;
                    continue;
                }
            }
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut out);
    out
}

/// Render markdown text into wrapped, styled lines. `indent` columns are prefixed.
pub fn markdown_lines(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    for raw in text.split('\n') {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            if !in_code {
                let lang = trimmed.trim_start_matches('`').trim();
                if !lang.is_empty() {
                    out.push(Line::styled(format!("  {lang}"), theme.muted));
                }
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            // Code is not word-wrapped; hard-split on width.
            let body = format!("  {raw}");
            for chunk in hard_split(&body, width) {
                out.push(Line::styled(chunk, theme.code_block));
            }
            continue;
        }
        if raw.trim().is_empty() {
            out.push(Line::default());
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let segs = vec![(trimmed[hashes..].trim().to_string(), theme.heading)];
            out.extend(wrap_spans(&segs, width));
            continue;
        }
        let (prefix, body) = if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            ("• ", rest)
        } else {
            ("", raw)
        };
        let mut segs = Vec::new();
        if !prefix.is_empty() {
            segs.push((prefix.to_string(), theme.accent));
        }
        segs.extend(inline_segments(body, theme.text, theme));
        out.extend(wrap_spans(&segs, width));
    }
    if out.is_empty() {
        out.push(Line::default());
    }
    out
}

/// Split a string into chunks of at most `width` display columns.
pub fn hard_split(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > width {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(ch);
        w += cw;
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn wraps_words() {
        let l = wrap_text("hello world foo bar", 10);
        assert_eq!(l, vec!["hello", "world foo", "bar"]);
    }

    #[test]
    fn wraps_long_word_and_newlines() {
        let l = wrap_text("abcdefghij\nxy", 4);
        assert_eq!(l, vec!["abcd", "efgh", "ij", "xy"]);
        for line in wrap_text("a b c d e f g h", 3) {
            assert!(UnicodeWidthStr::width(line.as_str()) <= 3);
        }
    }

    #[test]
    fn inline_styles() {
        let t = Theme::default();
        let s = inline_segments("a **b** `c` d", t.text, &t);
        assert_eq!(s.len(), 5);
        assert_eq!(s[1], ("b".to_string(), t.bold));
        assert_eq!(s[3], ("c".to_string(), t.code_inline));
    }

    #[test]
    fn markdown_blocks() {
        let t = Theme::default();
        let md = "# Title\n\nsome **bold** text\n- item\n```rust\nlet x = 1;\n```\nend";
        let lines = markdown_lines(md, 40, &t);
        let txt: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(txt[0], "Title");
        assert_eq!(txt[0].len(), 5);
        assert!(txt.contains(&"• item".to_string()));
        assert!(txt.contains(&"  let x = 1;".to_string()));
        assert!(txt.contains(&"  rust".to_string()));
        assert!(!txt.iter().any(|l| l.contains("```")));
        assert_eq!(lines[0].spans[0].style, t.heading);
        let code = lines.iter().find(|l| text(l) == "  let x = 1;").unwrap();
        assert_eq!(code.style, t.code_block);
    }

    #[test]
    fn markdown_wraps() {
        let t = Theme::default();
        let lines = markdown_lines("one two three four five six", 10, &t);
        assert!(lines.len() >= 3);
        assert!(lines.iter().all(|l| text(l).chars().count() <= 10));
    }
}
