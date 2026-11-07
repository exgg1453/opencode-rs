//! `opencode` binary entry point.

mod account;
mod cli;
mod format;
mod prompt;
mod run;
mod setup;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command};
use oc_core::SessionStore;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    match real_main().await {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}

fn env_list(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Token from `OPENCODE_SERVER_TOKEN`, else a fresh random one; extra hosts and origins
/// from `OPENCODE_PUBLIC_URL`, `OPENCODE_ALLOWED_HOSTS` and `OPENCODE_CORS_ORIGINS`.
fn server_security(addr: &std::net::SocketAddr) -> (oc_server::Security, String) {
    let token = std::env::var("OPENCODE_SERVER_TOKEN")
        .ok()
        .filter(|t| t.len() >= 16)
        .unwrap_or_else(oc_server::Security::generate_token);
    let mut sec = oc_server::Security::new(token.clone(), addr.ip().is_loopback());
    let public_host = std::env::var("OPENCODE_PUBLIC_URL")
        .ok()
        .and_then(|u| u.split("://").nth(1).map(|r| r.split(['/', ':']).next().unwrap_or("").to_string()))
        .filter(|h| !h.is_empty());
    for h in env_list("OPENCODE_ALLOWED_HOSTS").into_iter().chain(public_host) {
        sec = sec.allow_host(&h);
    }
    for o in env_list("OPENCODE_CORS_ORIGINS") {
        sec = sec.allow_origin(&o);
    }
    (sec, token)
}

async fn real_main() -> Result<bool> {
    let cli = Cli::parse();
    let cwd = setup::resolve_cwd(cli.cwd.clone())?;
    match cli.command {
        Some(Command::Models) => {
            account::run_models(&oc_core::auth::AuthStore::open_default());
            Ok(true)
        }
        Some(Command::Auth(c)) => {
            account::run_auth(c)?;
            Ok(true)
        }
        Some(Command::Share(a)) => {
            account::run_share(&cwd, a).await?;
            Ok(true)
        }
        Some(Command::Unshare(a)) => {
            account::run_unshare(&cwd, a).await?;
            Ok(true)
        }
        Some(Command::Export(a)) => {
            account::run_export(a)?;
            Ok(true)
        }
        Some(Command::Import(a)) => {
            account::run_import(a)?;
            Ok(true)
        }
        Some(Command::Trust(a)) => {
            if a.list {
                let dirs = oc_core::trust::list();
                if dirs.is_empty() {
                    println!("No trusted folders.");
                }
                for d in dirs {
                    println!("{d}");
                }
            } else if a.remove {
                if oc_core::trust::untrust(&cwd)? {
                    println!("Removed trust for {}", cwd.display());
                } else {
                    println!("{} was not trusted", cwd.display());
                }
            } else if oc_core::trust::trust(&cwd)? {
                println!("Trusted {}. Its plugins, MCP and LSP commands can now run.", cwd.display());
            } else {
                println!("{} is already trusted.", cwd.display());
            }
            Ok(true)
        }
        Some(Command::Sessions) => {
            let store = SessionStore::open_default()?;
            for s in store.list()? {
                println!("{}\t{}\t{} messages", s.id, s.title, s.messages.len());
            }
            Ok(true)
        }
        Some(Command::Run(args)) => {
            let agent = setup::build_agent(cli.model.as_deref(), cli.thinking, &cwd).await?;
            let store = SessionStore::open_default()?;
            run::run(agent, store, args, cwd).await
        }
        Some(Command::Serve(args)) => {
            let agent = setup::build_agent(cli.model.as_deref(), cli.thinking, &cwd).await?;
            let store = SessionStore::open_default()?;
            let addr: std::net::SocketAddr = format!("{}:{}", args.hostname, args.port)
                .parse()
                .context("invalid --hostname/--port")?;
            let (security, token) = server_security(&addr);
            eprintln!("opencode server listening on http://{addr}");
            eprintln!("API token: {token}");
            eprintln!("Send it as `Authorization: Bearer <token>` (or ?token= on /event).");
            if !addr.ip().is_loopback() {
                eprintln!("warning: listening on a non-loopback address; anyone who can reach it and has the token can run commands on this machine");
            }
            tokio::select! {
                r = oc_server::serve(agent, store, addr, security) => r?,
                _ = tokio::signal::ctrl_c() => eprintln!("\nshutting down"),
            }
            Ok(true)
        }
        Some(Command::Tui) | None => {
            let agent = setup::build_agent(cli.model.as_deref(), cli.thinking, &cwd).await?;
            let store = SessionStore::open_default()?;
            let session = oc_core::Session::new(cwd);
            oc_tui::run(agent, session, store).await?;
            Ok(true)
        }
    }
}
