//! `auth`, `share`, `unshare`, `export` and `import` subcommands.

use std::io::{BufRead, Write};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use oc_ai::{ContentBlock, Role};
use oc_core::auth::AuthStore;
use oc_core::{Config, Session, SessionStore};

/// Provider ids the provider registry knows, in its order.
fn known_provider_ids() -> Vec<&'static str> {
    oc_ai::known_providers().into_iter().map(|p| p.id).collect()
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Store an API key for a provider
    Login(LoginArgs),
    /// Remove the stored key for a provider
    Logout { provider: String },
    /// List providers with stored keys
    List,
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Provider id (e.g. anthropic); prompted if omitted
    pub provider: Option<String>,
}

#[derive(Debug, Args)]
pub struct IdArgs {
    /// Session id
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    Json,
    Md,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Session id
    pub session_id: String,
    #[arg(long, value_enum, default_value = "json")]
    pub format: ExportFormat,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Session JSON file produced by `export`
    pub file: std::path::PathBuf,
}

pub fn env_var_for(provider: &str) -> String {
    oc_ai::known_providers()
        .into_iter()
        .find(|p| p.id == provider)
        .and_then(|p| p.env_vars.first().map(|e| e.to_string()))
        .unwrap_or_else(|| format!("{}_API_KEY", provider.to_uppercase().replace(['-', '.'], "_")))
}

pub fn run_auth(cmd: AuthCommand) -> Result<()> {
    run_auth_with(cmd, &AuthStore::open_default(), &mut std::io::stdin().lock(), &|prompt| {
        rpassword::prompt_password(prompt).map_err(Into::into)
    })
}

pub fn run_auth_with(
    cmd: AuthCommand,
    store: &AuthStore,
    input: &mut dyn BufRead,
    ask_secret: &dyn Fn(&str) -> Result<String>,
) -> Result<()> {
    match cmd {
        AuthCommand::Login(a) => {
            let provider = match a.provider {
                Some(p) => p,
                None => {
                    println!("Known providers:");
                    for p in known_provider_ids() {
                        println!("  {p}");
                    }
                    print!("Provider: ");
                    std::io::stdout().flush()?;
                    let mut line = String::new();
                    input.read_line(&mut line)?;
                    line.trim().to_string()
                }
            };
            if provider.is_empty() {
                bail!("no provider given");
            }
            let key = ask_secret(&format!("API key for {provider}: "))?;
            store.set_api_key(&provider, &key)?;
            println!("Saved credentials for {provider} in {}", store.path().display());
        }
        AuthCommand::Logout { provider } => {
            if store.remove(&provider)? {
                println!("Removed credentials for {provider}");
            } else {
                bail!("no stored credentials for {provider}");
            }
        }
        AuthCommand::List => {
            let stored = store.list();
            println!("Stored credentials ({}):", store.path().display());
            if stored.is_empty() {
                println!("  (none)");
            }
            for p in &stored {
                println!("  {p}");
            }
            println!("Environment:");
            let mut names: Vec<String> = known_provider_ids().into_iter().map(str::to_string).collect();
            let extra: Vec<String> = stored.iter().filter(|p| !names.contains(p)).cloned().collect();
            names.extend(extra);
            for p in names {
                let env = env_var_for(&p);
                let set = std::env::var(&env).map(|v| !v.is_empty()).unwrap_or(false);
                println!("  {env}: {}", if set { "set" } else { "not set" });
            }
        }
    }
    Ok(())
}

/// `opencode models`: every provider the registry can build, and whether it has a key.
pub fn run_models(store: &AuthStore) {
    let stored = store.list();
    println!("{:<12} {:<16} KEY", "PROVIDER", "NAME");
    for p in oc_ai::known_providers() {
        let env = p.env_vars.iter().find(|e| std::env::var(e).is_ok_and(|v| !v.is_empty()));
        let key = if stored.iter().any(|s| s == p.id) {
            "stored (opencode auth login)".to_string()
        } else if let Some(e) = env {
            format!("from {e}")
        } else if p.env_vars.is_empty() {
            "not needed (local)".to_string()
        } else {
            format!("missing: set {} or run `opencode auth login {}`", p.env_vars[0], p.id)
        };
        println!("{:<12} {:<16} {key}", p.id, p.name);
    }
    println!();
    println!("Use a model as --model <provider>/<model>, e.g. --model {}", crate::cli::DEFAULT_MODEL);
    println!("Any other provider id works when `provider.<id>.options.baseURL` is set in opencode.json.");
}

fn load_session(store: &SessionStore, id: &str) -> Result<Session> {
    store.load(id).with_context(|| format!("session {id} not found"))
}

pub async fn run_share(cwd: &std::path::Path, args: IdArgs) -> Result<()> {
    let config = Config::load(cwd)?;
    let session = load_session(&SessionStore::open_default()?, &args.session_id)?;
    let url = oc_core::share::share_session(&config, &session).await?;
    println!("{url}");
    Ok(())
}

pub async fn run_unshare(cwd: &std::path::Path, args: IdArgs) -> Result<()> {
    let config = Config::load(cwd)?;
    oc_core::share::unshare_session(&config, &args.session_id).await?;
    eprintln!("Unshared session {}", args.session_id);
    Ok(())
}

pub fn export_session(session: &Session, format: ExportFormat) -> Result<String> {
    Ok(match format {
        ExportFormat::Json => serde_json::to_string_pretty(session)?,
        ExportFormat::Md => to_markdown(session),
    })
}

pub fn to_markdown(s: &Session) -> String {
    let mut out = format!("# {}\n\n", s.title);
    for m in &s.messages {
        out.push_str(match m.role {
            Role::User => "## User\n\n",
            Role::Assistant => "## Assistant\n\n",
        });
        for b in &m.content {
            match b {
                ContentBlock::Text { text } => {
                    out.push_str(text);
                    out.push_str("\n\n");
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let pretty = serde_json::to_string_pretty(input).unwrap_or_default();
                    out.push_str(&format!("**Tool call:** `{name}`\n\n```json\n{pretty}\n```\n\n"));
                }
                ContentBlock::ToolResult { content, is_error, .. } => {
                    let label = if *is_error { "Tool error" } else { "Tool result" };
                    out.push_str(&format!("**{label}:**\n\n```\n{content}\n```\n\n"));
                }
                ContentBlock::Thinking { thinking, .. } => {
                    out.push_str(&format!("> **Thinking:** {}\n\n", thinking.trim().replace('\n', "\n> ")));
                }
                ContentBlock::RedactedThinking { .. } => {}
            }
        }
    }
    out
}

pub fn run_export(args: ExportArgs) -> Result<()> {
    let session = load_session(&SessionStore::open_default()?, &args.session_id)?;
    println!("{}", export_session(&session, args.format)?);
    Ok(())
}

/// Imports a session file; assigns a fresh id if one with the same id already exists.
pub fn import_into(store: &SessionStore, file: &std::path::Path) -> Result<Session> {
    let data = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let mut session: Session = serde_json::from_str(&data).context("not a valid session file")?;
    if session.id.is_empty() || store.load(&session.id).is_ok() {
        session.id = Session::new(session.cwd.clone()).id;
    }
    store.save(&session)?;
    Ok(session)
}

pub fn run_import(args: ImportArgs) -> Result<()> {
    let s = import_into(&SessionStore::open_default()?, &args.file)?;
    println!("Imported session {} ({})", s.id, s.title);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_ai::ChatMessage;

    fn sample() -> Session {
        let mut s = Session::new("/tmp".into());
        s.title = "T".into();
        s.messages.push(ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: "hi".into() }] });
        s.messages.push(ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse { id: "1".into(), name: "bash".into(), input: serde_json::json!({"c":1}) }],
        });
        s
    }

    #[test]
    fn markdown_export() {
        let md = to_markdown(&sample());
        assert!(md.starts_with("# T\n"));
        assert!(md.contains("## User\n\nhi"));
        assert!(md.contains("`bash`"));
    }

    #[test]
    fn json_export_roundtrips_via_import() {
        let d = tempfile::tempdir().unwrap();
        let store = SessionStore::open(d.path().join("s")).unwrap();
        let s = sample();
        let file = d.path().join("x.json");
        std::fs::write(&file, export_session(&s, ExportFormat::Json).unwrap()).unwrap();
        let a = import_into(&store, &file).unwrap();
        assert_eq!(a.id, s.id);
        let b = import_into(&store, &file).unwrap();
        assert_ne!(b.id, s.id);
        assert_eq!(store.load(&b.id).unwrap().messages.len(), 2);
        std::fs::write(&file, "{}").unwrap();
        assert!(import_into(&store, &file).is_err());
    }

    #[test]
    fn auth_login_logout_list() {
        let d = tempfile::tempdir().unwrap();
        let store = AuthStore::in_dir(d.path());
        let ask = |_: &str| Ok("sk-secret".to_string());
        run_auth_with(AuthCommand::Login(LoginArgs { provider: Some("acme".into()) }), &store, &mut "".as_bytes(), &ask).unwrap();
        assert_eq!(store.get_api_key("acme").as_deref(), Some("sk-secret"));
        run_auth_with(AuthCommand::Login(LoginArgs { provider: None }), &store, &mut "anthropic\n".as_bytes(), &ask).unwrap();
        assert!(store.get_api_key("anthropic").is_some());
        run_auth_with(AuthCommand::List, &store, &mut "".as_bytes(), &ask).unwrap();
        run_auth_with(AuthCommand::Logout { provider: "acme".into() }, &store, &mut "".as_bytes(), &ask).unwrap();
        assert!(run_auth_with(AuthCommand::Logout { provider: "acme".into() }, &store, &mut "".as_bytes(), &ask).is_err());
        assert_eq!(env_var_for("my-prov"), "MY_PROV_API_KEY");
    }
}
