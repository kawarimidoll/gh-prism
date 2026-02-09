mod app;
mod git;
mod github;

use app::App;
use clap::Parser;
use color_eyre::Result;
use std::collections::HashMap;

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

fn resolve_repo(repo_arg: &Option<String>) -> Result<(String, String)> {
    // 1. --repo オプションが指定されていればそれを使う
    if let Some(repo) = repo_arg {
        let parts: Vec<&str> = repo.split('/').collect();
        if parts.len() == 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
        return Err(color_eyre::eyre::eyre!(
            "Invalid repo format. Use owner/repo"
        ));
    }

    // 2. gh repo view で自動検出
    let output = std::process::Command::new("gh")
        .args([
            "repo",
            "view",
            "--json",
            "owner,name",
            "-q",
            ".owner.login + \"/\" + .name",
        ])
        .output()?;

    if !output.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "Could not detect repository. Use --repo option"
        ));
    }

    let repo_str = String::from_utf8(output.stdout)?.trim().to_string();
    let parts: Vec<&str> = repo_str.split('/').collect();
    if parts.len() == 2 {
        Ok((parts[0].to_string(), parts[1].to_string()))
    } else {
        Err(color_eyre::eyre::eyre!("Could not parse repository info"))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    // リポジトリ情報を解決
    let (owner, repo) = resolve_repo(&cli.repo)?;

    // GitHub APIクライアントを作成してPR情報を取得
    let client = github::client::create_client()?;
    let pr = github::pr::fetch_pr(&client, &owner, &repo, cli.pr_number).await?;

    // PRタイトルを取得（Option<String>なのでunwrap_or_default）
    let pr_title = pr.title.unwrap_or_default();

    // コミット一覧を取得
    let commits = github::commits::fetch_commits(&client, &owner, &repo, cli.pr_number).await?;

    // 全コミットのファイルを事前取得
    let mut files_map: HashMap<String, Vec<github::files::DiffFile>> = HashMap::new();
    for commit in &commits {
        let files = github::files::fetch_commit_files(&client, &owner, &repo, &commit.sha).await?;
        files_map.insert(commit.sha.clone(), files);
    }

    let terminal = ratatui::init();
    let mut app = App::new(
        cli.pr_number,
        format!("{}/{}", owner, repo),
        pr_title,
        commits,
        files_map,
        Some(client),
    );
    let result = app.run(terminal);
    ratatui::restore();
    result
}
