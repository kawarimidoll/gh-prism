mod app;
mod git;
mod github;

use app::App;
use clap::Parser;
use color_eyre::Result;
use futures::stream::{FuturesUnordered, StreamExt};
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
    eprintln!("Fetching PR #{}...", cli.pr_number);
    let pr = github::pr::fetch_pr(&client, &owner, &repo, cli.pr_number).await?;

    // PR情報を取得（Option<String>なのでunwrap_or_default）
    let pr_title = pr.title.unwrap_or_default();
    let pr_body = pr.body.unwrap_or_default();

    // コミット一覧を取得
    eprintln!("Fetching commits...");
    let commits = github::commits::fetch_commits(&client, &owner, &repo, cli.pr_number).await?;

    // 全コミットのファイルを並列取得
    let total = commits.len();
    eprintln!("Fetching files for {} commits...", total);

    // 全コミットの初期状態を表示
    for commit in &commits {
        eprintln!("  ⏳ {} {}", commit.short_sha(), commit.message_summary());
    }

    // 並列フェッチを開始
    let futs: FuturesUnordered<_> = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            let client = client.clone();
            let owner = owner.clone();
            let repo = repo.clone();
            let sha = commit.sha.clone();
            async move {
                let result = github::files::fetch_commit_files(&client, &owner, &repo, &sha).await;
                (i, sha, result)
            }
        })
        .collect();

    let mut files_map: HashMap<String, Vec<github::files::DiffFile>> = HashMap::new();
    futures::pin_mut!(futs);
    while let Some((idx, sha, result)) = futs.next().await {
        let files = result?;
        files_map.insert(sha, files);

        // ANSI エスケープでカーソルを該当行に移動して更新
        let up = total - idx;
        eprint!("\x1b[{}A\r\x1b[2K", up);
        eprintln!(
            "  ✅ {} {}",
            commits[idx].short_sha(),
            commits[idx].message_summary()
        );
        let down = up.saturating_sub(1);
        if down > 0 {
            eprint!("\x1b[{}B", down);
        }
    }

    let terminal = ratatui::init();
    let mut app = App::new(
        cli.pr_number,
        format!("{}/{}", owner, repo),
        pr_title,
        pr_body,
        commits,
        files_map,
        Some(client),
    );
    let result = app.run(terminal);
    ratatui::restore();
    result
}
