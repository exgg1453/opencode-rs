//! Headless `run` command.

use std::io::{IsTerminal, Read, Write};

use anyhow::{bail, Result};
use std::sync::Arc;

use oc_core::permission::{AllowAll, Asker, DenyAll, RulePermission};
use oc_core::{Agent, AgentEvent, Config, Session, SessionStore};
use tokio::sync::mpsc;

use crate::cli::{OutputFormat, RunArgs};
use crate::prompt::{TtyAsker, NON_INTERACTIVE_REASON};
use crate::format::{format_event, format_json, Sink};

/// Build the prompt from args, falling back to stdin.
pub fn join_message(parts: &[String], stdin_text: impl FnOnce() -> String, stdin_is_tty: bool) -> String {
    let wants_stdin = parts.is_empty() && !stdin_is_tty || parts.len() == 1 && parts[0] == "-";
    if wants_stdin {
        stdin_text().trim().to_string()
    } else {
        parts.join(" ")
    }
}

fn read_stdin() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().read_to_string(&mut s);
    s
}

/// Wire `RulePermission` into the agent with the right asker for this run.
fn install_permissions(agent: &mut Agent, args: &RunArgs, cwd: &std::path::Path, interactive: bool) {
    let config = Config::load(cwd).unwrap_or_default();
    let asker: Arc<dyn Asker> = if args.yes {
        Arc::new(AllowAll)
    } else if interactive {
        Arc::new(TtyAsker::new())
    } else {
        Arc::new(DenyAll(NON_INTERACTIVE_REASON.to_string()))
    };
    agent.hooks.permission = Some(Arc::new(RulePermission::new(&config, asker).with_cwd(cwd)));
}

/// Returns true if the run succeeded.
pub async fn run(mut agent: Agent, store: SessionStore, args: RunArgs, cwd: std::path::PathBuf) -> Result<bool> {
    let message = join_message(&args.message, read_stdin, std::io::stdin().is_terminal());
    if message.is_empty() {
        bail!("no message provided");
    }

    install_permissions(&mut agent, &args, &cwd, std::io::stdin().is_terminal());

    let mut session = if let Some(id) = &args.session {
        store.load(id)?
    } else if args.continue_last {
        match store.list()?.into_iter().next_back() {
            Some(s) => s,
            None => bail!("no session to continue"),
        }
    } else {
        Session::new(cwd)
    };

    let json = args.format == OutputFormat::Json;
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let printer = tokio::spawn(async move {
        let mut failed = false;
        let mut out = std::io::stdout();
        let mut err = std::io::stderr();
        while let Some(ev) = rx.recv().await {
            if matches!(ev, AgentEvent::Error { .. }) {
                failed = true;
            }
            if json {
                let _ = writeln!(out, "{}", format_json(&ev));
            } else if let Some((sink, text)) = format_event(&ev) {
                match sink {
                    Sink::Stdout => {
                        let _ = write!(out, "{text}");
                    }
                    Sink::Stderr => {
                        let _ = write!(err, "{text}");
                    }
                }
            }
            let _ = out.flush();
        }
        failed
    });

    let mut cancelled = false;
    let mut prompt_err = None;
    tokio::select! {
        r = agent.prompt(&mut session, message, tx) => {
            if let Err(e) = r { prompt_err = Some(e); }
        }
        _ = tokio::signal::ctrl_c() => { cancelled = true; }
    }
    // `tx` was moved into prompt; its drop (or the cancelled future's) closes the channel.
    let event_error = printer.await.unwrap_or(true);

    if let Err(e) = store.save(&session) {
        eprintln!("warning: failed to save session: {e:#}");
    }
    if cancelled {
        eprintln!("\ncancelled");
        return Ok(false);
    }
    if let Some(e) = prompt_err {
        eprintln!("error: {e:#}");
        return Ok(false);
    }
    Ok(!event_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_args() {
        let m = join_message(&["a".into(), "b".into()], || panic!("no stdin"), false);
        assert_eq!(m, "a b");
    }

    #[test]
    fn dash_reads_stdin() {
        assert_eq!(join_message(&["-".into()], || " hello \n".into(), true), "hello");
    }

    #[test]
    fn absent_reads_stdin_only_when_piped() {
        assert_eq!(join_message(&[], || "piped".into(), false), "piped");
        assert_eq!(join_message(&[], || panic!("tty"), true), "");
    }
}
