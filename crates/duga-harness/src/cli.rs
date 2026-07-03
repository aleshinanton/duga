use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "duga-harness",
    about = "Run the duga agent loop",
    after_help = "Examples:\n  duga-harness --config duga.yaml \"write tests\"\n  duga-harness --config duga.yaml --model dummy/test --task \"echo hello\""
)]
pub struct Cli {
    #[arg(long, required_unless_present = "login")]
    pub config: Option<PathBuf>,
    /// Run an interactive OAuth login for a provider (e.g. "anthropic") and exit.
    #[arg(long, value_name = "PROVIDER")]
    pub login: Option<String>,
    #[arg(long)]
    pub task: Option<String>,
    #[arg(long)]
    pub provider: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long, default_value = "./replays")]
    pub replay_dir: PathBuf,
    #[cfg(feature = "tui")]
    #[arg(long)]
    pub tui: bool,
    #[arg(short, long)]
    pub verbose: bool,
    #[arg(value_name = "TASK")]
    pub positional_task: Option<String>,
}

impl Cli {
    #[cfg(test)]
    pub fn parse_args<I, T>(args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        Self::parse_from(args)
    }

    pub fn task_text(&self) -> Option<&str> {
        self.task.as_deref().or(self.positional_task.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_positional_task() {
        let cli = Cli::parse_args(["duga-harness", "--config", "cfg.yaml", "do work"]);
        assert_eq!(cli.config, Some(PathBuf::from("cfg.yaml")));
        assert_eq!(cli.task_text(), Some("do work"));
    }

    #[test]
    fn parses_login_without_config() {
        let cli = Cli::parse_args(["duga-harness", "--login", "anthropic"]);
        assert_eq!(cli.login.as_deref(), Some("anthropic"));
        assert_eq!(cli.config, None);
    }

    #[test]
    fn parses_flag_task_and_model_override() {
        let cli = Cli::parse_args([
            "duga-harness",
            "--config",
            "cfg.yaml",
            "--task",
            "do work",
            "--model",
            "dummy/other",
            "--provider",
            "dummy",
        ]);
        assert_eq!(cli.task_text(), Some("do work"));
        assert_eq!(cli.model.as_deref(), Some("dummy/other"));
        assert_eq!(cli.provider.as_deref(), Some("dummy"));
    }
}
