//! Command line definition.

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

pub const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-5-5";

#[derive(Debug, Parser)]
#[command(name = "opencode", version, about = "AI coding agent for the terminal")]
pub struct Cli {
    /// Model as "provider/model" (overrides config)
    #[arg(short, long, global = true)]
    pub model: Option<String>,
    /// Working directory
    #[arg(long, global = true)]
    pub cwd: Option<PathBuf>,
    /// Model reasoning effort (overrides config)
    #[arg(long, global = true, value_enum, value_name = "LEVEL")]
    pub thinking: Option<ThinkingArg>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThinkingArg {
    Off,
    Low,
    Medium,
    High,
}

impl ThinkingArg {
    pub fn level(self) -> Option<oc_ai::ThinkingLevel> {
        match self {
            ThinkingArg::Off => None,
            ThinkingArg::Low => Some(oc_ai::ThinkingLevel::Low),
            ThinkingArg::Medium => Some(oc_ai::ThinkingLevel::Medium),
            ThinkingArg::High => Some(oc_ai::ThinkingLevel::High),
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a single prompt headlessly
    Run(RunArgs),
    /// Start the interactive TUI (default)
    Tui,
    /// Start the HTTP API server
    Serve(ServeArgs),
    /// List saved sessions
    Sessions,
    /// List known providers and models
    Models,
    /// Manage stored provider credentials
    #[command(subcommand)]
    Auth(crate::account::AuthCommand),
    /// Share a session and print its URL
    Share(crate::account::IdArgs),
    /// Stop sharing a session
    Unshare(crate::account::IdArgs),
    /// Export a session to stdout
    Export(crate::account::ExportArgs),
    /// Import a session from a JSON file
    Import(crate::account::ImportArgs),
    /// Trust this folder so its project-defined plugins/MCP/LSP commands may run
    Trust(TrustArgs),
}

#[derive(Debug, clap::Args)]
pub struct TrustArgs {
    /// Show all trusted folders instead of trusting this one
    #[arg(long)]
    pub list: bool,
    /// Remove trust for this folder
    #[arg(long)]
    pub remove: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Default,
    Json,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Message to send ("-" or absent reads stdin)
    pub message: Vec<String>,
    /// Output format
    #[arg(long, value_enum, default_value = "default")]
    pub format: OutputFormat,
    /// Resume the session with this id
    #[arg(short, long)]
    pub session: Option<String>,
    /// Continue the most recent session
    #[arg(short = 'c', long = "continue")]
    pub continue_last: bool,
    /// Auto-approve every permission prompt (deny rules still apply)
    #[arg(short = 'y', long = "yes", visible_alias = "dangerously-skip-permissions")]
    pub yes: bool,
    /// Do not print the model's reasoning
    #[arg(long)]
    pub hide_thinking: bool,
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    #[arg(long, default_value_t = 4096)]
    pub port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    pub hostname: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("opencode").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn no_subcommand() {
        let c = p(&[]);
        assert!(c.command.is_none());
    }

    #[test]
    fn global_flags_anywhere() {
        let c = p(&["run", "-m", "openai/gpt-5", "--cwd", "/tmp", "hi"]);
        assert_eq!(c.model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(c.cwd, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn run_args() {
        let c = p(&["run", "--format", "json", "-s", "abc", "fix", "the", "bug"]);
        let Some(Command::Run(r)) = c.command else { panic!() };
        assert_eq!(r.message, vec!["fix", "the", "bug"]);
        assert_eq!(r.format, OutputFormat::Json);
        assert_eq!(r.session.as_deref(), Some("abc"));
        assert!(!r.continue_last);
    }

    #[test]
    fn run_continue_and_stdin() {
        let c = p(&["run", "--continue", "-"]);
        let Some(Command::Run(r)) = c.command else { panic!() };
        assert!(r.continue_last);
        assert_eq!(r.message, vec!["-"]);
        assert_eq!(r.format, OutputFormat::Default);
    }

    #[test]
    fn serve_defaults_and_overrides() {
        let Some(Command::Serve(s)) = p(&["serve"]).command else { panic!() };
        assert_eq!((s.port, s.hostname.as_str()), (4096, "127.0.0.1"));
        let Some(Command::Serve(s)) = p(&["serve", "--port", "1", "--hostname", "0.0.0.0"]).command else { panic!() };
        assert_eq!((s.port, s.hostname.as_str()), (1, "0.0.0.0"));
    }

    #[test]
    fn simple_subcommands() {
        assert!(matches!(p(&["tui"]).command, Some(Command::Tui)));
        assert!(matches!(p(&["sessions"]).command, Some(Command::Sessions)));
        assert!(matches!(p(&["models"]).command, Some(Command::Models)));
    }

    #[test]
    fn bad_format_rejected() {
        assert!(Cli::try_parse_from(["opencode", "run", "--format", "xml", "x"]).is_err());
    }

    #[test]
    fn thinking_flags() {
        assert_eq!(p(&["run", "hi"]).thinking, None);
        for (v, want) in [("off", None), ("low", Some(oc_ai::ThinkingLevel::Low)), ("medium", Some(oc_ai::ThinkingLevel::Medium)), ("high", Some(oc_ai::ThinkingLevel::High))] {
            let c = p(&["--thinking", v, "run", "hi"]);
            assert_eq!(c.thinking.unwrap().level(), want);
            // global: also accepted after the subcommand
            assert_eq!(p(&["run", "--thinking", v, "hi"]).thinking.unwrap().level(), want);
        }
        assert!(Cli::try_parse_from(["opencode", "--thinking", "max"]).is_err());
        let Some(Command::Run(a)) = p(&["run", "--hide-thinking", "hi"]).command else { panic!() };
        assert!(a.hide_thinking);
        let Some(Command::Run(a)) = p(&["run", "hi"]).command else { panic!() };
        assert!(!a.hide_thinking);
    }

    #[test]
    fn yes_flag_and_alias() {
        for flag in ["--yes", "-y", "--dangerously-skip-permissions"] {
            match p(&["run", flag, "hi"]).command {
                Some(Command::Run(a)) => assert!(a.yes, "{flag}"),
                _ => panic!("not run"),
            }
        }
        match p(&["run", "hi"]).command {
            Some(Command::Run(a)) => assert!(!a.yes),
            _ => panic!("not run"),
        }
    }
}
