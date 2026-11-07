use std::path::Path;

use oc_core::{AgentEvent, Config, ToolContext, ToolRegistry};
use serde_json::json;

fn cfg(extra: serde_json::Value) -> Config {
    Config { plugin: extra.as_object().unwrap().clone(), ..Default::default() }
}

#[tokio::test]
async fn example_plugin_end_to_end() {
    let bin = env!("CARGO_BIN_EXE_example_plugin");
    let log = std::env::temp_dir().join(format!("oc-ext-events-{}", std::process::id()));
    let c = cfg(json!({
        "example": {"command":[bin], "environment":{"EXAMPLE_PLUGIN_EVENT_LOG": log.to_string_lossy()}},
        "off": {"command":[bin], "enabled": false},
        "broken": {"command":["/nonexistent/plugin"]}
    }));
    let mut reg = ToolRegistry::default();
    let hooks = oc_ext::load_plugins_with_tools(&c, Path::new("/nonexistent-cwd"), &mut reg, true).await.unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(reg.tools.len(), 1);
    let t = &reg.tools[0];
    assert_eq!(t.name(), "current_time");
    let out = t.execute(json!({}), &ToolContext { cwd: std::env::temp_dir() }).await.unwrap();
    assert!(out.parse::<u64>().unwrap() > 1_000_000);

    let h = &hooks[0];
    let mut ok = json!({"command":"ls -la"});
    h.before_tool("bash", &mut ok).await.unwrap();
    let mut bad = json!({"command":"sudo  rm -rf /"});
    let err = h.before_tool("bash", &mut bad).await.unwrap_err();
    assert!(err.to_string().contains("refusing dangerous command"), "{err}");
    // other tools unaffected
    h.before_tool("read", &mut bad).await.unwrap();

    let mut o = "hello".to_string();
    h.after_tool("bash", &ok, &mut o).await.unwrap();
    assert_eq!(o, "hello\n[checked by example_plugin]");

    h.on_event("s1", &AgentEvent::Done).await;
    for _ in 0..50 {
        if std::fs::read_to_string(&log).map(|s| s.contains("done")).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "done");
    std::fs::remove_file(&log).ok();
}

#[tokio::test]
async fn crashing_plugin_is_disabled_not_fatal() {
    // Exits right after the handshake line is consumed: handshake fails -> skipped.
    let c = cfg(json!({"dead": {"command":["sh","-c","exit 0"]}}));
    let hooks = oc_ext::load_plugins(&c, Path::new(".")).await.unwrap();
    assert!(hooks.is_empty());

    // Answers initialize, then dies: hooks must keep working (allow) afterwards.
    let script = r#"read l; echo '{"jsonrpc":"2.0","id":1,"result":{"hooks":["tool.execute.before","tool.execute.after"]}}'; read l2; exit 0"#;
    let c = cfg(json!({"flaky": {"command":["sh","-c",script], "timeout": 500}}));
    let hooks = oc_ext::load_plugins(&c, Path::new(".")).await.unwrap();
    assert_eq!(hooks.len(), 1);
    let mut input = json!({"a":1});
    hooks[0].before_tool("bash", &mut input).await.unwrap();
    assert_eq!(input, json!({"a":1}));
    let mut out = "x".to_string();
    hooks[0].after_tool("bash", &input, &mut out).await.unwrap();
    assert_eq!(out, "x");
}

#[tokio::test]
async fn plugin_dir_discovery() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let cwd = std::env::temp_dir().join(format!("oc-ext-cwd-{}", std::process::id()));
        let dir = cwd.join(".opencode/plugins");
        std::fs::create_dir_all(&dir).unwrap();
        let dst = dir.join("ex");
        std::fs::copy(env!("CARGO_BIN_EXE_example_plugin"), &dst).unwrap();
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a plugin").unwrap();
        let mut reg = ToolRegistry::default();
        let hooks = oc_ext::load_plugins_with_tools(&Config::default(), &cwd, &mut reg, true).await.unwrap();
        std::fs::remove_dir_all(&cwd).ok();
        assert_eq!(hooks.len(), 1);
        assert_eq!(reg.tools.len(), 1);
    }
}
