//! Server table: built-ins plus `config.lsp` overrides.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ServerDef {
    pub name: String,
    pub command: Vec<String>,
    /// Extensions without the dot.
    pub extensions: Vec<String>,
    pub root_markers: Vec<String>,
}

fn def(name: &str, cmd: &[&str], exts: &[&str], roots: &[&str]) -> ServerDef {
    ServerDef {
        name: name.into(),
        command: cmd.iter().map(|s| s.to_string()).collect(),
        extensions: exts.iter().map(|s| s.to_string()).collect(),
        root_markers: roots.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn builtin() -> Vec<ServerDef> {
    vec![
        def("rust-analyzer", &["rust-analyzer"], &["rs"], &["Cargo.toml"]),
        def(
            "typescript",
            &["typescript-language-server", "--stdio"],
            &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"],
            &["package.json", "tsconfig.json"],
        ),
        def(
            "pyright",
            &["pyright-langserver", "--stdio"],
            &["py", "pyi"],
            &["pyproject.toml", "setup.py", "requirements.txt", "pyrightconfig.json"],
        ),
        def("pylsp", &["pylsp"], &["py"], &["pyproject.toml", "setup.py", "requirements.txt"]),
        def("gopls", &["gopls"], &["go"], &["go.mod", "go.work"]),
        def(
            "clangd",
            &["clangd"],
            &["c", "cc", "cpp", "cxx", "h", "hh", "hpp"],
            &["compile_commands.json", "CMakeLists.txt", ".clangd"],
        ),
    ]
}

/// Build the table. `None` result means LSP is disabled entirely.
pub fn table(lsp: Option<&Value>) -> Option<Vec<ServerDef>> {
    let mut defs = builtin();
    match lsp {
        Some(Value::Bool(false)) => return None,
        Some(Value::Object(map)) => {
            for (name, v) in map {
                let disabled = v.get("disabled").and_then(Value::as_bool).unwrap_or(false);
                if disabled {
                    defs.retain(|d| &d.name != name);
                    continue;
                }
                let strs = |k: &str| -> Option<Vec<String>> {
                    v.get(k)?.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                };
                let command = strs("command");
                let exts = strs("extensions").map(|e| e.into_iter().map(|x| x.trim_start_matches('.').to_string()).collect());
                if let Some(d) = defs.iter_mut().find(|d| &d.name == name) {
                    if let Some(c) = command {
                        d.command = c;
                    }
                    if let Some(e) = exts {
                        d.extensions = e;
                    }
                    if let Some(r) = strs("root_markers") {
                        d.root_markers = r;
                    }
                } else if let Some(c) = command.filter(|c| !c.is_empty()) {
                    defs.insert(
                        0,
                        ServerDef {
                            name: name.clone(),
                            command: c,
                            extensions: exts.unwrap_or_default(),
                            root_markers: strs("root_markers").unwrap_or_default(),
                        },
                    );
                }
            }
        }
        _ => {}
    }
    Some(defs)
}

pub fn language_id(ext: &str) -> &'static str {
    match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hh" | "hpp" => "cpp",
        _ => "plaintext",
    }
}

/// `which`-style lookup.
pub fn find_binary(cmd: &str) -> Option<PathBuf> {
    if cmd.contains('/') {
        let p = PathBuf::from(cmd);
        return p.is_file().then_some(p);
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|d| d.join(cmd)).find(|p| p.is_file())
}

pub fn find_root(file: &Path, markers: &[String], cwd: &Path) -> PathBuf {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if markers.iter().any(|m| d.join(m).exists()) {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    cwd.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn disabled_and_overrides() {
        assert!(table(Some(&json!(false))).is_none());
        let t = table(Some(&json!({
            "gopls": {"disabled": true},
            "rust-analyzer": {"command": ["/x/ra"]},
            "mine": {"command": ["m"], "extensions": [".foo"]}
        })))
        .unwrap();
        assert!(!t.iter().any(|d| d.name == "gopls"));
        assert_eq!(t.iter().find(|d| d.name == "rust-analyzer").unwrap().command, vec!["/x/ra"]);
        assert_eq!(t[0].name, "mine");
        assert_eq!(t[0].extensions, vec!["foo"]);
    }

    #[test]
    fn root_detection() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/src")).unwrap();
        std::fs::write(d.path().join("a/Cargo.toml"), "").unwrap();
        let f = d.path().join("a/src/m.rs");
        assert_eq!(find_root(&f, &["Cargo.toml".into()], d.path()), d.path().join("a"));
        assert_eq!(find_root(&f, &["nope".into()], d.path()), d.path());
    }
}
