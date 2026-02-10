mod app;
mod git;
mod github;

use app::{App, ThemeMode};
use clap::Parser;
use color_eyre::Result;
use futures::stream::{FuturesUnordered, StreamExt};
use github::cache::PrCache;
use github::commits::CommitInfo;
use github::files::DiffFile;
use octocrab::Octocrab;
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

    /// Disable cache and always fetch from API
    #[arg(long)]
    no_cache: bool,

    /// Force light theme
    #[arg(long, conflicts_with = "dark")]
    light: bool,

    /// Force dark theme
    #[arg(long, conflicts_with = "light")]
    dark: bool,
}

/// termbg でターミナル背景色を検出し、ライト/ダークモードを判定する。
/// 検出失敗時はダークモードにフォールバック。
fn detect_theme() -> ThemeMode {
    match termbg::theme(std::time::Duration::from_millis(100)) {
        Ok(termbg::Theme::Light) => ThemeMode::Light,
        _ => ThemeMode::Dark,
    }
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

/// PR情報とコミットごとのファイルをAPI経由で全取得する
async fn fetch_all(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
    commits: &[CommitInfo],
) -> Result<(String, String, HashMap<String, Vec<DiffFile>>)> {
    // PR情報を取得
    let pr = github::pr::fetch_pr(client, owner, repo, pr_number).await?;
    let pr_title = pr.title.unwrap_or_default();
    let pr_body = pr.body.unwrap_or_default();

    // 全コミットのファイルを並列取得
    let total = commits.len();
    eprintln!("Fetching files for {} commits...", total);

    for commit in commits {
        eprintln!("  ⏳ {} {}", commit.short_sha(), commit.message_summary());
    }

    let futs: FuturesUnordered<_> = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            let client = client.clone();
            let owner = owner.to_string();
            let repo = repo.to_string();
            let sha = commit.sha.clone();
            async move {
                let result = github::files::fetch_commit_files(&client, &owner, &repo, &sha).await;
                (i, sha, result)
            }
        })
        .collect();

    let mut files_map: HashMap<String, Vec<DiffFile>> = HashMap::new();
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

    Ok((pr_title, pr_body, files_map))
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    // リポジトリ情報を解決
    let (owner, repo) = resolve_repo(&cli.repo)?;

    // GitHub APIクライアントを作成
    let client = github::client::create_client()?;
    eprintln!("Fetching PR #{}...", cli.pr_number);

    // コミット一覧を常にAPI取得（HEAD SHA判定に必要）
    let commits = github::commits::fetch_commits(&client, &owner, &repo, cli.pr_number).await?;
    let head_sha = commits.last().map(|c| c.sha.as_str()).unwrap_or("");

    // キャッシュ判定 + レビューコメント取得を並列実行
    let data_future = async {
        if !cli.no_cache {
            if let Some(cached) = github::cache::read_cache(&owner, &repo, cli.pr_number) {
                if cached.head_sha == head_sha {
                    eprintln!(
                        "Using cached data (HEAD: {})",
                        &head_sha[..7.min(head_sha.len())]
                    );
                    return Ok((cached.pr_title, cached.pr_body, cached.files_map));
                }
                eprintln!(
                    "Cache stale (expected {}, got {})",
                    &cached.head_sha[..7.min(cached.head_sha.len())],
                    &head_sha[..7.min(head_sha.len())]
                );
            } else {
                eprintln!("No cache found, fetching from API...");
            }
        } else {
            eprintln!("Cache disabled, fetching from API...");
        }
        let (title, body, fmap) =
            fetch_all(&client, &owner, &repo, cli.pr_number, &commits).await?;
        github::cache::write_cache(
            &owner,
            &repo,
            cli.pr_number,
            &PrCache {
                head_sha: head_sha.to_string(),
                pr_title: title.clone(),
                pr_body: body.clone(),
                commits: commits.clone(),
                files_map: fmap.clone(),
            },
        );
        Ok((title, body, fmap))
    };

    let comments_future =
        github::comments::fetch_review_comments(&client, &owner, &repo, cli.pr_number);

    let ((pr_title, pr_body, files_map), review_comments) =
        tokio::try_join!(data_future, comments_future)?;

    // テーマ検出（ratatui::init() の前に実行 — raw mode では OSC クエリが動かない）
    let theme = if cli.light {
        ThemeMode::Light
    } else if cli.dark {
        ThemeMode::Dark
    } else {
        detect_theme()
    };

    let terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;

    let mut app = App::new(
        cli.pr_number,
        format!("{}/{}", owner, repo),
        pr_title,
        pr_body,
        commits,
        files_map,
        review_comments,
        Some(client),
        theme,
    );
    let result = app.run(terminal);

    crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
    ratatui::restore();
    result
}
