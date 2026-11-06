use oc_core::Config;
use oc_lsp::start;
use serde_json::json;

fn fake_cfg() -> Config {
    Config { lsp: Some(json!({"fake": {"command": [env!("CARGO_BIN_EXE_fake_lsp")], "extensions": ["fk"]}})), ..Default::default() }
}

#[tokio::test]
async fn e2e_fake_server() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.fk");
    std::fs::write(&f, "ok\nfine\n").unwrap();
    let p = start(&fake_cfg(), dir.path()).await.unwrap();
    assert_eq!(p.diagnostics(&f).await, None);
    std::fs::write(&f, "ok\n    BAD\n").unwrap();
    assert_eq!(p.diagnostics(&f).await.unwrap(), "<file_diagnostics>\nERROR [2:5] bad token\n</file_diagnostics>");
    std::fs::write(&f, "ok\n").unwrap();
    assert_eq!(p.diagnostics(&f).await, None);
    // Unknown extension and missing binary are silently skipped.
    let g = dir.path().join("b.rs");
    std::fs::write(&g, "x").unwrap();
    let cfg = Config { lsp: Some(json!({"rust-analyzer": {"command": ["/nonexistent/ra"]}})), ..Default::default() };
    assert_eq!(start(&cfg, dir.path()).await.unwrap().diagnostics(&g).await, None);
}
