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
    #[command(subcommand)]
    pub command: Option<Command>,
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
}
