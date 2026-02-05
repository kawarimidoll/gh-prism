mod app;
mod github;

use app::App;
use clap::Parser;
use color_eyre::Result;

#[derive(Parser)]
#[command(name = "prism")]
#[command(about = "A TUI tool for reviewing GitHub Pull Requests")]
struct Cli {
    /// Pull Request number
    pr_number: u64,

    /// Repository in owner/repo format (default: detect from git remote)
    #[arg(short, long)]
    repo: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let terminal = ratatui::init();
    let result = App::new(cli.pr_number, cli.repo).run(terminal);
    ratatui::restore();
    result
}
