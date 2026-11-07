//! Self-contained read-only HTML rendering of a shared session.

use oc_ai::{ContentBlock, Role};
use oc_core::Session;

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

/// Inline markdown (`code`, **bold**, *italic*) applied to already-escaped text.
fn inline(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('`') {
            if let Some(end) = r.find('`') {
                out.push_str(&format!("<code>{}</code>", &r[..end]));
                rest = &r[end + 1..];
                continue;
            }
        }
        if let Some(r) = rest.strip_prefix("**") {
            if let Some(end) = r.find("**").filter(|&e| e > 0) {
                out.push_str(&format!("<strong>{}</strong>", &r[..end]));
                rest = &r[end + 2..];
                continue;
            }
        }
        if let Some(r) = rest.strip_prefix('*') {
            if let Some(end) = r.find('*').filter(|&e| e > 0 && !r.starts_with(' ')) {
                out.push_str(&format!("<em>{}</em>", &r[..end]));
                rest = &r[end + 1..];
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Basic markdown: fenced code, headings, bullet lists, paragraphs. Input is raw text.
pub fn markdown(text: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    let mut in_list = false;
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, out: &mut String| {
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", para.join("<br>")));
            para.clear();
        }
    };
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            flush(&mut para, &mut out);
            if in_list {
                out.push_str("</ul>\n");
                in_list = false;
            }
            out.push_str(if in_code { "</code></pre>\n" } else { "<pre><code>" });
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push_str(&escape(line));
            out.push('\n');
            continue;
        }
        let esc = escape(line);
        let trimmed = esc.trim_start();
        let bullet = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* "));
        if let Some(item) = bullet {
            flush(&mut para, &mut out);
            if !in_list {
                out.push_str("<ul>\n");
                in_list = true;
            }
            out.push_str(&format!("<li>{}</li>\n", inline(item)));
            continue;
        }
        if in_list {
            out.push_str("</ul>\n");
            in_list = false;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut para, &mut out);
            out.push_str(&format!("<h{0}>{1}</h{0}>\n", hashes + 2, inline(trimmed[hashes + 1..].trim())));
        } else if trimmed.is_empty() {
            flush(&mut para, &mut out);
        } else {
            para.push(inline(trimmed));
        }
    }
    flush(&mut para, &mut out);
    if in_list {
        out.push_str("</ul>\n");
    }
    if in_code {
        out.push_str("</code></pre>\n");
    }
    out
}

pub fn render(session: &Session) -> String {
    let mut body = String::new();
    for m in &session.messages {
        let (cls, who) = match m.role {
            Role::User => ("user", "You"),
            Role::Assistant => ("assistant", "Assistant"),
        };
        let mut inner = String::new();
        for block in &m.content {
            match block {
                ContentBlock::Text { text } => inner.push_str(&format!("<div class=\"md\">{}</div>", markdown(text))),
                ContentBlock::ToolUse { name, input, .. } => {
                    let pretty = serde_json::to_string_pretty(input).unwrap_or_default();
                    inner.push_str(&format!(
                        "<details class=\"tool\"><summary>Tool call: <code>{}</code></summary><pre><code>{}</code></pre></details>",
                        escape(name),
                        escape(&pretty)
                    ));
                }
                // Reasoning is not published with shared sessions.
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
                ContentBlock::ToolResult { content, is_error, .. } => {
                    let label = if *is_error { "Tool error" } else { "Tool result" };
                    let cls = if *is_error { "tool err" } else { "tool" };
                    inner.push_str(&format!(
                        "<details class=\"{cls}\"><summary>{label}</summary><pre><code>{}</code></pre></details>",
                        escape(content)
                    ));
                }
            }
        }
        body.push_str(&format!("<section class=\"msg {cls}\"><div class=\"who\">{who}</div>{inner}</section>\n"));
    }
    if session.messages.is_empty() {
        body.push_str("<p class=\"empty\">This session has no messages.</p>");
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"color-scheme\" content=\"light dark\"><meta name=\"robots\" content=\"noindex\">\
<title>{title}</title><style>{css}</style></head><body><main><header><h1>{title}</h1>\
<p class=\"meta\">Shared OpenCode session (read-only)</p></header>\n{body}</main></body></html>\n",
        title = escape(&session.title),
        css = CSS,
        body = body
    )
}

const CSS: &str = ":root{--bg:#fafafa;--fg:#1c1c1e;--muted:#6b6b70;--card:#fff;--border:#e2e2e6;--user:#eef4ff;--code:#f1f1f4;--err:#c62828}\
@media (prefers-color-scheme:dark){:root{--bg:#121214;--fg:#e8e8ea;--muted:#9a9aa2;--card:#1b1b1e;--border:#2e2e33;--user:#182236;--code:#25252a;--err:#ef7b7b}}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.55 system-ui,-apple-system,Segoe UI,sans-serif}\
main{max-width:50rem;margin:0 auto;padding:1.5rem 1rem 4rem}h1{font-size:1.4rem;margin:.2rem 0}.meta{color:var(--muted);margin:0 0 1.5rem;font-size:.9rem}\
.msg{background:var(--card);border:1px solid var(--border);border-radius:10px;padding:.75rem 1rem;margin:.9rem 0}.msg.user{background:var(--user)}\
.who{font-size:.75rem;text-transform:uppercase;letter-spacing:.06em;color:var(--muted);margin-bottom:.3rem}\
.md p{margin:.4rem 0}.md h3,.md h4,.md h5{margin:.8rem 0 .3rem}pre{background:var(--code);padding:.7rem;border-radius:6px;overflow-x:auto;font-size:.85rem}\
code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;background:var(--code);padding:.05rem .3rem;border-radius:4px;font-size:.88em}pre code{background:none;padding:0}\
details.tool{border:1px solid var(--border);border-radius:6px;margin:.5rem 0;padding:.3rem .6rem}details.err summary{color:var(--err)}\
summary{cursor:pointer;font-size:.88rem;color:var(--muted)}.empty{color:var(--muted)}";

#[cfg(test)]
mod tests {
    use super::*;
    use oc_ai::ChatMessage;

    #[test]
    fn escapes_everything() {
        assert_eq!(escape("<script>\"&'"), "&lt;script&gt;&quot;&amp;&#39;");
        let html = markdown("<img src=x onerror=alert(1)> **b** `<c>`\n```\n<x>\n```");
        assert!(!html.contains("<img"));
        assert!(html.contains("<strong>b</strong>"));
        assert!(html.contains("<code>&lt;c&gt;</code>"));
        assert!(html.contains("&lt;x&gt;"));
    }

    #[test]
    fn renders_session() {
        let mut s = Session::new("/tmp".into());
        s.title = "<b>t</b>".into();
        s.messages.push(ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: "hi <x>".into() }] });
        s.messages.push(ChatMessage {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolUse { id: "1".into(), name: "bash".into(), input: serde_json::json!({"command":"<ls>"}) },
                ContentBlock::ToolResult { tool_use_id: "1".into(), content: "a&b".into(), is_error: false },
            ],
        });
        let h = render(&s);
        assert!(h.contains("&lt;b&gt;t&lt;/b&gt;"));
        assert!(h.contains("hi &lt;x&gt;"));
        assert!(h.contains("<details class=\"tool\">"));
        assert!(h.contains("a&amp;b"));
        assert!(h.contains("prefers-color-scheme"));
        assert!(!h.contains("<script"));
    }

    #[test]
    fn lists_and_headings() {
        let h = markdown("# T\n- a\n- b\n\npara");
        assert!(h.contains("<h3>T</h3>") && h.contains("<ul>") && h.contains("<li>b</li>") && h.contains("<p>para</p>"));
    }
}
