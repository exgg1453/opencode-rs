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

const PROVIDERS: &[(&str, &str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY", "anthropic/claude-sonnet-5-5"),
    ("openai", "OPENAI_API_KEY", "openai/gpt-5"),
];

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

async fn real_main() -> Result<bool> {
    let cli = Cli::parse();
    let cwd = setup::resolve_cwd(cli.cwd.clone())?;
    match cli.command {
        Some(Command::Models) => {
            for (name, env, example) in PROVIDERS {
                let set = if std::env::var(env).is_ok() { "configured" } else { "no key" };
                println!("{name}\t{example}\t({env}: {set})");
            }
            println!("default model: {}", cli::DEFAULT_MODEL);
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
        Some(Command::Sessions) => {
            let store = SessionStore::open_default()?;
            for s in store.list()? {
                println!("{}\t{}\t{} messages", s.id, s.title, s.messages.len());
            }
            Ok(true)
        }
        Some(Command::Run(args)) => {
            let agent = setup::build_agent(cli.model.as_deref(), &cwd).await?;
            let store = SessionStore::open_default()?;
            run::run(agent, store, args, cwd).await
        }
        Some(Command::Serve(args)) => {
            let agent = setup::build_agent(cli.model.as_deref(), &cwd).await?;
            let store = SessionStore::open_default()?;
            let addr: std::net::SocketAddr = format!("{}:{}", args.hostname, args.port)
                .parse()
                .context("invalid --hostname/--port")?;
            eprintln!("opencode server listening on http://{addr}");
            tokio::select! {
                r = oc_server::serve(agent, store, addr) => r?,
                _ = tokio::signal::ctrl_c() => eprintln!("\nshutting down"),
            }
            Ok(true)
        }
        Some(Command::Tui) | None => {
            let agent = setup::build_agent(cli.model.as_deref(), &cwd).await?;
            let store = SessionStore::open_default()?;
            let session = oc_core::Session::new(cwd);
            oc_tui::run(agent, session, store).await?;
            Ok(true)
        }
    }
}
