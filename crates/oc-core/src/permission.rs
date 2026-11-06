//! Config-driven permission rules (`permission` section of opencode.json).
//! Owner: permission agent. Keep these signatures; UIs implement `Asker`.
//!
//! Model (after the original OpenCode): a flat, ordered list of rules
//! `(permission, pattern, action)`; the LAST rule whose permission and pattern both
//! match wins; no match means "ask". Built-in defaults come first, then the user's
//! config, so config overrides defaults.
//!
//! Config shape: `{ "bash": { "*": "ask", "git *": "allow" }, "edit": "ask", "read": "allow" }`.
//! A string value is shorthand for `{ "*": value }`. Object entries keep their written
//! order (a lone `"*"` entry is always treated as the base, i.e. evaluated first).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use crate::{Config, PermissionDecision, PermissionHandler, PermissionRequest};

/// Asks the user interactively (TUI dialog, CLI prompt, server client).
#[async_trait]
pub trait Asker: Send + Sync {
    async fn ask(&self, req: &PermissionRequest) -> PermissionDecision;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Allow,
    Ask,
    Deny,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        match s {
            "allow" => Some(Action::Allow),
            "ask" => Some(Action::Ask),
            "deny" => Some(Action::Deny),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub permission: String,
    pub pattern: String,
    pub action: Action,
}

/// Glob match: `*` matches any run of characters (including `/`), `?` matches one.
/// A trailing `" *"` also matches the bare command (`"git *"` matches `"git"`).
pub fn wildcard_match(text: &str, pattern: &str) -> bool {
    if glob(text.as_bytes(), pattern.as_bytes()) {
        return true;
    }
    if let Some(base) = pattern.strip_suffix(" *") {
        return glob(text.as_bytes(), base.as_bytes());
    }
    false
}

fn glob(t: &[u8], p: &[u8]) -> bool {
    let (mut ti, mut pi) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) && p[pi] != b'*' {
            ti += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Last matching rule wins; `None` when nothing matches.
pub fn evaluate(rules: &[Rule], permission: &str, resource: &str) -> Option<Action> {
    rules
        .iter()
        .rev()
        .find(|r| wildcard_match(permission, &r.permission) && wildcard_match(resource, &r.pattern))
        .map(|r| r.action)
}

/// Built-in defaults, close to the original: everything allowed except
/// directories outside the project, `.env` files and doom loops.
pub fn default_rules() -> Vec<Rule> {
    let r = |p: &str, pat: &str, a| Rule { permission: p.into(), pattern: pat.into(), action: a };
    vec![
        r("*", "*", Action::Allow),
        r("doom_loop", "*", Action::Ask),
        r("external_directory", "*", Action::Ask),
        r("read", "*", Action::Allow),
        r("read", "*.env", Action::Ask),
        r("read", "*.env.*", Action::Ask),
        r("read", "*.env.example", Action::Allow),
    ]
}

/// Parse the `permission` config section into rules (defaults NOT included).
pub fn parse_config(section: &serde_json::Map<String, Value>) -> Vec<Rule> {
    let mut out = Vec::new();
    for (perm, v) in section {
        match v {
            Value::String(s) => {
                if let Some(a) = Action::parse(s) {
                    out.push(Rule { permission: perm.clone(), pattern: "*".into(), action: a });
                }
            }
            Value::Object(m) => {
                // Base `*` first, the rest in written order.
                let mut entries: Vec<(&String, Action)> =
                    m.iter().filter_map(|(k, v)| v.as_str().and_then(Action::parse).map(|a| (k, a))).collect();
                entries.sort_by_key(|(k, _)| k.as_str() != "*");
                for (pat, a) in entries {
                    out.push(Rule { permission: perm.clone(), pattern: pat.clone(), action: a });
                }
            }
            _ => {}
        }
    }
    // A top-level "*" is the base for every permission: evaluate it before specific keys.
    out.sort_by_key(|r| !(r.permission == "*"));
    out
}

/// Lexically normalize `p` against `cwd` (no symlink resolution).
pub fn normalize_path(cwd: &Path, p: &str) -> PathBuf {
    let p = if let Some(rest) = p.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(rest),
            None => PathBuf::from(p),
        }
    } else {
        PathBuf::from(p)
    };
    let joined = if p.is_absolute() { p } else { cwd.join(p) };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// Split a shell command into simple commands (on `; & | newline ( )` and `$(`/backticks
/// outside single quotes), so every part is checked on its own.
pub fn split_commands(cmd: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq) = (false, false);
    let mut chars = cmd.chars().peekable();
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
            ';' | '&' | '|' | '\n' | '(' | ')' | '`' if !sq && (!dq || matches!(c, '(' | ')' | '`')) => {
                if !cur.trim().is_empty() {
                    parts.push(cur.trim().to_string());
                }
                cur.clear();
            }
            '$' if !sq && chars.peek() == Some(&'(') => {
                if !cur.trim().is_empty() {
                    parts.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

/// The "always" pattern for one simple command: `git commit *`, `ls *`, `npm run dev *`.
pub fn command_pattern(simple: &str) -> String {
    let words: Vec<&str> =
        simple.split_whitespace().skip_while(|w| w.contains('=') && !w.starts_with('-') && !w.starts_with('/')).collect();
    let Some(first) = words.first() else { return "*".into() };
    let arity = match *first {
        "npm" | "pnpm" | "yarn" | "bun" if words.get(1) == Some(&"run") => 3,
        "git" | "npm" | "pnpm" | "yarn" | "bun" | "cargo" | "docker" | "kubectl" | "go" | "pip" | "brew"
        | "systemctl" | "gh" => 2,
        _ => 1,
    };
    let take: Vec<&str> = words.iter().take(arity).copied().filter(|w| !w.starts_with('-')).collect();
    format!("{} *", take.join(" "))
}

/// One thing a tool call needs permission for.
#[derive(Debug, Clone)]
struct Item {
    permission: String,
    resource: String,
    /// Pattern remembered by "always".
    always: String,
    action: Action,
}

#[derive(Default)]
struct SessionState {
    approved: Vec<(String, String)>,
    recent: Vec<String>,
}

/// Applies the configured rules and calls the `Asker` when a rule says "ask".
pub struct RulePermission {
    asker: Arc<dyn Asker>,
    rules: Vec<Rule>,
    cwd: PathBuf,
    sessions: Mutex<HashMap<String, SessionState>>,
}

impl RulePermission {
    pub fn new(config: &Config, asker: Arc<dyn Asker>) -> RulePermission {
        let mut rules = default_rules();
        rules.extend(parse_config(&config.permission));
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        RulePermission { asker, rules, cwd, sessions: Mutex::new(HashMap::new()) }
    }

    /// Directory that counts as "inside the project" (default: process cwd).
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    fn eval(&self, permission: &str, resource: &str) -> Action {
        evaluate(&self.rules, permission, resource).unwrap_or(Action::Ask)
    }

    fn display_path(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.cwd).unwrap_or(abs).to_string_lossy().into_owned()
    }

    fn items(&self, req: &PermissionRequest) -> Vec<Item> {
        let str_of = |k: &str| req.input.get(k).and_then(Value::as_str);
        let mut items = Vec::new();
        let tool = req.tool.as_str();
        match tool {
            "bash" => {
                let cmd = str_of("command").unwrap_or("");
                let segs = split_commands(cmd);
                let segs = if segs.is_empty() { vec![cmd.trim().to_string()] } else { segs };
                for s in segs {
                    items.push(Item {
                        permission: "bash".into(),
                        action: self.eval("bash", &s),
                        always: command_pattern(&s),
                        resource: s,
                    });
                }
            }
            "read" | "edit" | "write" | "patch" | "multiedit" | "list" | "glob" | "grep" => {
                let perm = if matches!(tool, "edit" | "write" | "patch" | "multiedit") { "edit" } else { tool };
                let raw = str_of("file_path").or_else(|| str_of("filePath")).or_else(|| str_of("path")).unwrap_or(".");
                let abs = normalize_path(&self.cwd, raw);
                let rel = self.display_path(&abs);
                let external = !abs.starts_with(&self.cwd);
                items.push(Item {
                    permission: perm.into(),
                    action: self.eval(perm, &rel),
                    always: if perm == "read" && rel.contains(".env") { rel.clone() } else { "*".into() },
                    resource: rel,
                });
                if external {
                    let dir = if matches!(tool, "list" | "glob" | "grep") || abs.is_dir() {
                        abs.clone()
                    } else {
                        abs.parent().map(Path::to_path_buf).unwrap_or_else(|| abs.clone())
                    };
                    let dir = dir.to_string_lossy().into_owned();
                    let resource = format!("{}/*", dir.trim_end_matches('/'));
                    items.push(Item {
                        permission: "external_directory".into(),
                        action: self.eval("external_directory", &resource),
                        always: resource.clone(),
                        resource,
                    });
                }
            }
            other => {
                let resource = str_of("url").or_else(|| str_of("query")).unwrap_or("*").to_string();
                items.push(Item {
                    permission: other.into(),
                    action: self.eval(other, &resource),
                    always: "*".into(),
                    resource,
                });
            }
        }
        items
    }

    /// Records the call and reports whether it repeats the previous two identically.
    fn is_doom_loop(&self, req: &PermissionRequest) -> bool {
        let key = format!("{}\u{0}{}", req.tool, req.input);
        let mut map = self.sessions.lock().unwrap();
        let st = map.entry(req.session_id.clone()).or_default();
        st.recent.push(key);
        if st.recent.len() > 3 {
            st.recent.remove(0);
        }
        st.recent.len() == 3 && st.recent.iter().all(|k| *k == st.recent[0])
    }

    fn approved(&self, session: &str, item: &Item) -> bool {
        let map = self.sessions.lock().unwrap();
        map.get(session).is_some_and(|st| {
            st.approved.iter().any(|(p, pat)| *p == item.permission && wildcard_match(&item.resource, pat))
        })
    }

    fn remember(&self, session: &str, items: &[&Item]) {
        let mut map = self.sessions.lock().unwrap();
        let st = map.entry(session.to_string()).or_default();
        for i in items {
            let e = (i.permission.clone(), i.always.clone());
            if !st.approved.contains(&e) {
                st.approved.push(e);
            }
        }
    }
}

#[async_trait]
impl PermissionHandler for RulePermission {
    async fn check(&self, req: &PermissionRequest) -> PermissionDecision {
        let mut items = self.items(req);
        if self.is_doom_loop(req) {
            items.push(Item {
                permission: "doom_loop".into(),
                resource: req.tool.clone(),
                always: "*".into(),
                action: self.eval("doom_loop", &req.tool),
            });
        }
        if let Some(i) = items.iter().find(|i| i.action == Action::Deny) {
            return PermissionDecision::Deny {
                reason: Some(format!(
                    "denied by permission rule ({} \"{}\"); the configuration forbids this action",
                    i.permission, i.resource
                )),
            };
        }
        let pending: Vec<&Item> =
            items.iter().filter(|i| i.action == Action::Ask && !self.approved(&req.session_id, i)).collect();
        if pending.is_empty() {
            return PermissionDecision::Allow;
        }
        match self.asker.ask(req).await {
            PermissionDecision::AllowAlways => {
                self.remember(&req.session_id, &pending);
                PermissionDecision::Allow
            }
            d => d,
        }
    }
}

/// An asker that approves everything (for `--yes`).
pub struct AllowAll;

#[async_trait]
impl Asker for AllowAll {
    async fn ask(&self, _req: &PermissionRequest) -> PermissionDecision {
        PermissionDecision::Allow
    }
}

/// An asker that refuses everything with a reason (non-interactive runs).
pub struct DenyAll(pub String);

#[async_trait]
impl Asker for DenyAll {
    async fn ask(&self, _req: &PermissionRequest) -> PermissionDecision {
        PermissionDecision::Deny { reason: Some(self.0.clone()) }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(AtomicUsize, PermissionDecision);

    #[async_trait]
    impl Asker for Counting {
        async fn ask(&self, _req: &PermissionRequest) -> PermissionDecision {
            self.0.fetch_add(1, Ordering::SeqCst);
            self.1.clone()
        }
    }

    fn cfg(v: Value) -> Config {
        Config { permission: v.as_object().unwrap().clone(), ..Default::default() }
    }

    fn handler(v: Value, answer: PermissionDecision) -> (RulePermission, Arc<Counting>) {
        let a = Arc::new(Counting(AtomicUsize::new(0), answer));
        (RulePermission::new(&cfg(v), a.clone()).with_cwd("/proj"), a)
    }

    fn req(tool: &str, input: Value) -> PermissionRequest {
        PermissionRequest { session_id: "s1".into(), call_id: "c".into(), tool: tool.into(), input }
    }

    fn asks(a: &Counting) -> usize {
        a.0.load(Ordering::SeqCst)
    }

    #[test]
    fn wildcard_basics() {
        assert!(wildcard_match("git status", "git *"));
        assert!(wildcard_match("git", "git *"));
        assert!(!wildcard_match("gitx", "git *"));
        assert!(wildcard_match("a/b/.env", "*.env"));
        assert!(wildcard_match("abc", "a?c"));
        assert!(!wildcard_match("abcd", "a?c"));
        assert!(wildcard_match("anything", "*"));
        assert!(wildcard_match("", "*"));
        assert!(!wildcard_match("x", ""));
    }

    #[test]
    fn last_match_wins_and_config_shapes() {
        let rules = parse_config(&cfg(json!({"bash": {"git push *": "deny", "*": "ask", "git *": "allow"}})).permission);
        // "*" is moved first; the rest keep written order (preserve_order or not, sorted maps differ),
        // so check via evaluate on rules we build explicitly.
        assert!(rules.iter().any(|r| r.pattern == "*"));
        let explicit = vec![
            Rule { permission: "bash".into(), pattern: "*".into(), action: Action::Ask },
            Rule { permission: "bash".into(), pattern: "git *".into(), action: Action::Allow },
            Rule { permission: "bash".into(), pattern: "git push *".into(), action: Action::Deny },
        ];
        assert_eq!(evaluate(&explicit, "bash", "git status"), Some(Action::Allow));
        assert_eq!(evaluate(&explicit, "bash", "git push origin"), Some(Action::Deny));
        assert_eq!(evaluate(&explicit, "bash", "rm x"), Some(Action::Ask));
        assert_eq!(evaluate(&explicit, "edit", "x"), None);
    }

    #[test]
    fn string_shorthand_and_ignored_garbage() {
        let rules = parse_config(&cfg(json!({"edit": "ask", "bash": "bogus", "x": 3})).permission);
        assert_eq!(rules, vec![Rule { permission: "edit".into(), pattern: "*".into(), action: Action::Ask }]);
    }

    #[test]
    fn top_level_star_is_base() {
        let rules = parse_config(&cfg(json!({"bash": "allow", "*": "ask"})).permission);
        assert_eq!(rules[0].permission, "*");
    }

    #[test]
    fn splits_commands() {
        assert_eq!(split_commands("ls -la && git status | grep x; echo $(whoami)"), vec![
            "ls -la", "git status", "grep x", "echo", "whoami"
        ]);
        assert_eq!(split_commands("echo 'a && b'"), vec!["echo 'a && b'"]);
        assert_eq!(split_commands("echo \"a; b\""), vec!["echo \"a; b\""]);
        assert_eq!(split_commands("echo `id`"), vec!["echo", "id"]);
    }

    #[test]
    fn command_patterns() {
        assert_eq!(command_pattern("ls -la /tmp"), "ls *");
        assert_eq!(command_pattern("git commit -m x"), "git commit *");
        assert_eq!(command_pattern("npm run dev --port 1"), "npm run dev *");
        assert_eq!(command_pattern("FOO=1 cargo test"), "cargo test *");
        assert_eq!(command_pattern(""), "*");
    }

    #[test]
    fn normalizes_paths() {
        let cwd = Path::new("/proj");
        assert_eq!(normalize_path(cwd, "a/../b/./c"), PathBuf::from("/proj/b/c"));
        assert_eq!(normalize_path(cwd, "../x"), PathBuf::from("/x"));
        assert_eq!(normalize_path(cwd, "/etc/passwd"), PathBuf::from("/etc/passwd"));
    }

    #[tokio::test]
    async fn defaults_allow_normal_work_without_asking() {
        let (h, a) = handler(json!({}), PermissionDecision::Deny { reason: None });
        for r in [
            req("bash", json!({"command": "rm -rf build"})),
            req("write", json!({"file_path": "src/x.rs", "content": ""})),
            req("read", json!({"file_path": "/proj/src/x.rs"})),
            req("webfetch", json!({"url": "http://x"})),
        ] {
            assert_eq!(h.check(&r).await, PermissionDecision::Allow, "{}", r.tool);
        }
        assert_eq!(asks(&a), 0);
    }

    #[tokio::test]
    async fn env_files_ask_but_example_allowed() {
        let (h, a) = handler(json!({}), PermissionDecision::Allow);
        assert_eq!(h.check(&req("read", json!({"file_path": ".env"}))).await, PermissionDecision::Allow);
        assert_eq!(asks(&a), 1);
        h.check(&req("read", json!({"file_path": "cfg/.env.local"}))).await;
        assert_eq!(asks(&a), 2);
        h.check(&req("read", json!({"file_path": ".env.example"}))).await;
        assert_eq!(asks(&a), 2);
    }

    #[tokio::test]
    async fn external_directory_asks_and_always_remembers_dir() {
        let (h, a) = handler(json!({}), PermissionDecision::AllowAlways);
        let r = req("read", json!({"file_path": "/etc/hosts"}));
        assert_eq!(h.check(&r).await, PermissionDecision::Allow);
        assert_eq!(asks(&a), 1);
        // Same directory: remembered.
        h.check(&req("read", json!({"file_path": "/etc/passwd"}))).await;
        assert_eq!(asks(&a), 1);
        // Different directory: asks again.
        h.check(&req("read", json!({"file_path": "/var/log/x"}))).await;
        assert_eq!(asks(&a), 2);
        // `..` escape is detected.
        h.check(&req("write", json!({"file_path": "../other/f"}))).await;
        assert_eq!(asks(&a), 3);
    }

    #[tokio::test]
    async fn external_directory_deny() {
        let (h, a) = handler(json!({"external_directory": "deny"}), PermissionDecision::Allow);
        let d = h.check(&req("edit", json!({"file_path": "/etc/x"}))).await;
        assert!(matches!(d, PermissionDecision::Deny { .. }));
        assert_eq!(asks(&a), 0);
    }

    #[tokio::test]
    async fn bash_rules_deny_beats_allow_and_ask() {
        let (h, a) = handler(
            json!({"bash": {"*": "ask", "git *": "allow", "git push *": "deny"}}),
            PermissionDecision::Allow,
        );
        // json maps may be sorted alphabetically without preserve_order; the "*" base is moved first
        // and "git *" < "git push *" alphabetically, so the written order is reproduced either way.
        assert_eq!(h.check(&req("bash", json!({"command": "git status"}))).await, PermissionDecision::Allow);
        assert_eq!(asks(&a), 0);
        let d = h.check(&req("bash", json!({"command": "git push origin"}))).await;
        assert!(matches!(d, PermissionDecision::Deny { reason: Some(r) } if r.contains("bash")));
        h.check(&req("bash", json!({"command": "make"}))).await;
        assert_eq!(asks(&a), 1);
    }

    #[tokio::test]
    async fn compound_commands_check_every_part() {
        let (h, a) = handler(json!({"bash": {"*": "ask", "ls *": "allow", "rm *": "deny"}}), PermissionDecision::Allow);
        assert_eq!(h.check(&req("bash", json!({"command": "ls && ls -l"}))).await, PermissionDecision::Allow);
        assert_eq!(asks(&a), 0);
        let d = h.check(&req("bash", json!({"command": "ls; rm -rf /"}))).await;
        assert!(matches!(d, PermissionDecision::Deny { .. }));
        h.check(&req("bash", json!({"command": "ls $(curl x)"}))).await;
        assert_eq!(asks(&a), 1);
    }

    #[tokio::test]
    async fn always_for_bash_remembers_pattern_per_session() {
        let (h, a) = handler(json!({"bash": "ask"}), PermissionDecision::AllowAlways);
        h.check(&req("bash", json!({"command": "git commit -m one"}))).await;
        assert_eq!(asks(&a), 1);
        h.check(&req("bash", json!({"command": "git commit -m two"}))).await;
        assert_eq!(asks(&a), 1);
        h.check(&req("bash", json!({"command": "git push"}))).await;
        assert_eq!(asks(&a), 2);
        // Another session is unaffected.
        let mut other = req("bash", json!({"command": "git commit -m x"}));
        other.session_id = "s2".into();
        h.check(&other).await;
        assert_eq!(asks(&a), 3);
    }

    #[tokio::test]
    async fn once_does_not_remember_and_deny_passes_through() {
        let (h, a) = handler(json!({"edit": "ask"}), PermissionDecision::Allow);
        let r = req("edit", json!({"file_path": "a", "old_string": "", "new_string": ""}));
        h.check(&r).await;
        h.check(&r).await;
        assert_eq!(asks(&a), 2);
        let (h, _) = handler(json!({"edit": "ask"}), PermissionDecision::Deny { reason: Some("no".into()) });
        assert_eq!(h.check(&r).await, PermissionDecision::Deny { reason: Some("no".into()) });
    }

    #[tokio::test]
    async fn edit_covers_write_and_always_applies_to_both() {
        let (h, a) = handler(json!({"edit": "ask"}), PermissionDecision::AllowAlways);
        h.check(&req("write", json!({"file_path": "a", "content": ""}))).await;
        h.check(&req("edit", json!({"file_path": "b"}))).await;
        assert_eq!(asks(&a), 1);
    }

    #[tokio::test]
    async fn edit_path_patterns() {
        let (h, a) = handler(
            json!({"edit": {"*": "allow", "*.lock": "deny", "src/*": "ask"}}),
            PermissionDecision::Allow,
        );
        assert!(matches!(h.check(&req("write", json!({"file_path": "Cargo.lock"}))).await, PermissionDecision::Deny { .. }));
        h.check(&req("write", json!({"file_path": "/proj/src/a.rs"}))).await;
        assert_eq!(asks(&a), 1);
        h.check(&req("write", json!({"file_path": "docs/a.md"}))).await;
        assert_eq!(asks(&a), 1);
    }

    #[tokio::test]
    async fn tool_level_deny_and_global_star() {
        let (h, _) = handler(json!({"webfetch": "deny"}), PermissionDecision::Allow);
        assert!(matches!(h.check(&req("webfetch", json!({}))).await, PermissionDecision::Deny { .. }));
        let (h, a) = handler(json!({"*": "ask"}), PermissionDecision::Allow);
        h.check(&req("mcp_foo", json!({}))).await;
        assert_eq!(asks(&a), 1);
    }

    #[tokio::test]
    async fn doom_loop_asks_on_third_identical_call() {
        let (h, a) = handler(json!({}), PermissionDecision::Allow);
        let r = req("bash", json!({"command": "false"}));
        h.check(&r).await;
        h.check(&r).await;
        assert_eq!(asks(&a), 0);
        h.check(&r).await;
        assert_eq!(asks(&a), 1);
        // A different call breaks the streak.
        h.check(&req("bash", json!({"command": "true"}))).await;
        h.check(&r).await;
        assert_eq!(asks(&a), 1);
    }

    #[tokio::test]
    async fn askers_allow_all_and_deny_all() {
        let r = req("bash", json!({}));
        assert_eq!(AllowAll.ask(&r).await, PermissionDecision::Allow);
        assert_eq!(DenyAll("x".into()).ask(&r).await, PermissionDecision::Deny { reason: Some("x".into()) });
    }
}
