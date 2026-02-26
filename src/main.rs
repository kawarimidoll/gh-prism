mod app;
mod git;
mod github;

use app::{App, CodeCommentReply, ConversationEntry, ConversationKind, ThemeMode};
use clap::Parser;
use color_eyre::Result;
use futures::stream::{FuturesUnordered, StreamExt};
use github::comments::{IssueComment, ReviewComment, ReviewThread};
use github::commits::CommitInfo;
use github::files::DiffFile;
use github::media::MediaCache;
use github::review::ReviewSummary;
use octocrab::Octocrab;
use octocrab::models::pulls::PullRequest;
use std::collections::HashMap;

const SHORT_SHA_LEN: usize = 7;
const THEME_DETECT_TIMEOUT_MS: u64 = 100;

pub struct PrMetadata {
    pub pr_title: String,
    pub pr_body: String,
    pub pr_author: String,
    pub pr_base_branch: String,
    pub pr_head_branch: String,
    pub pr_created_at: String,
    pub pr_state: String,
}

pub fn extract_pr_metadata(pr: &PullRequest) -> PrMetadata {
    PrMetadata {
        pr_title: pr.title.clone().unwrap_or_default(),
        pr_body: pr.body.clone().unwrap_or_default(),
        pr_author: pr
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default(),
        pr_base_branch: pr.base.ref_field.clone(),
        pr_head_branch: pr.head.ref_field.clone(),
        pr_created_at: pr
            .created_at
            .map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M %z")
                    .to_string()
            })
            .unwrap_or_default(),
        pr_state: if pr.merged_at.is_some() {
            "Merged".to_string()
        } else {
            match pr.state {
                Some(octocrab::models::IssueState::Open) => "Open".to_string(),
                _ => "Closed".to_string(),
            }
        },
    }
}

/// 非同期エラーの発生元
pub enum AsyncErrorKind {
    Files,
    Conversation,
    Media,
}

/// バックグラウンド非同期タスクから App に送信するデータ
pub enum AsyncData {
    FilesMap(HashMap<String, Vec<DiffFile>>),
    ConversationData {
        review_comments: Vec<ReviewComment>,
        issue_comments: Vec<IssueComment>,
        reviews: Vec<ReviewSummary>,
        review_threads: Vec<ReviewThread>,
    },
    MediaData(MediaCache),
    Error(AsyncErrorKind, String),
}

const VERSION: &str = match option_env!("GH_PRISM_VERSION") {
    Some(v) => v,
    None => env!("DEV_VERSION"),
};

#[derive(Parser)]
#[command(name = "prism", version = VERSION)]
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
    match termbg::theme(std::time::Duration::from_millis(THEME_DETECT_TIMEOUT_MS)) {
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

/// 現在の認証ユーザーのログイン名を取得
pub fn fetch_current_user() -> String {
    std::process::Command::new("gh")
        .args(["api", "user", "-q", ".login"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// コミットごとのファイルをAPI経由で全取得して返す
/// `quiet` が true の場合は進捗表示を抑制する（TUI リロード時に使用）
pub async fn fetch_all(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    commits: &[CommitInfo],
    quiet: bool,
) -> Result<HashMap<String, Vec<DiffFile>>> {
    // 全コミットのファイルを並列取得
    let total = commits.len();
    if !quiet {
        eprintln!("Fetching files for {} commits...", total);
        for commit in commits {
            eprintln!("  ⏳ {} {}", commit.short_sha(), commit.message_summary());
        }
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

        if !quiet {
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
    }

    Ok(files_map)
}

/// IssueComment, ReviewSummary, ReviewComment を ConversationEntry にマージして時系列ソート
pub fn build_conversation(
    issue_comments: Vec<IssueComment>,
    reviews: Vec<ReviewSummary>,
    review_comments: Vec<ReviewComment>,
    review_threads: &[ReviewThread],
) -> Vec<ConversationEntry> {
    // root_comment_database_id → ReviewThread のルックアップマップ
    let thread_lookup: HashMap<u64, &ReviewThread> = review_threads
        .iter()
        .map(|t| (t.root_comment_database_id, t))
        .collect();
    let mut entries = Vec::new();

    for c in issue_comments {
        entries.push(ConversationEntry {
            author: c.user.login,
            body: c.body.unwrap_or_default(),
            created_at: c.created_at,
            kind: ConversationKind::IssueComment,
        });
    }

    for r in reviews {
        // submitted_at が None のレビューは未送信（下書き）なのでスキップ
        let Some(submitted_at) = r.submitted_at else {
            continue;
        };
        let body = r.body.as_deref().unwrap_or("");
        // body 空かつ state が COMMENTED のみの review はスキップ（空コメントノイズ防止）
        if body.is_empty() && r.state == "COMMENTED" {
            continue;
        }
        entries.push(ConversationEntry {
            author: r.user.login,
            body: body.to_string(),
            created_at: submitted_at,
            kind: ConversationKind::Review { state: r.state },
        });
    }

    // ReviewComment をスレッドごとにグルーピング
    // in_reply_to_id が None のものがルートコメント、Some のものがリプライ
    let mut root_comments: Vec<&ReviewComment> = Vec::new();
    let mut replies_map: HashMap<u64, Vec<&ReviewComment>> = HashMap::new();

    for rc in &review_comments {
        if let Some(parent_id) = rc.in_reply_to_id {
            replies_map.entry(parent_id).or_default().push(rc);
        } else {
            root_comments.push(rc);
        }
    }

    for root in root_comments {
        let mut replies = Vec::new();
        if let Some(thread_replies) = replies_map.get(&root.id) {
            let mut sorted_replies: Vec<&&ReviewComment> = thread_replies.iter().collect();
            sorted_replies.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            for r in sorted_replies {
                replies.push(CodeCommentReply {
                    author: r.user.login.clone(),
                    body: r.body.clone(),
                    created_at: r.created_at.clone(),
                });
            }
        }

        let thread_info = thread_lookup.get(&root.id);
        entries.push(ConversationEntry {
            author: root.user.login.clone(),
            body: root.body.clone(),
            created_at: root.created_at.clone(),
            kind: ConversationKind::CodeComment {
                path: root.path.clone(),
                line: root.line,
                replies,
                is_resolved: thread_info.is_some_and(|t| t.is_resolved),
                thread_node_id: thread_info.map(|t| t.node_id.clone()),
                root_comment_id: root.id,
            },
        });
    }

    // created_at で時系列ソート
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    entries
}

pub struct ReloadedData {
    pub metadata: PrMetadata,
    pub commits: Vec<CommitInfo>,
    pub files_map: HashMap<String, Vec<DiffFile>>,
    pub review_comments: Vec<ReviewComment>,
    pub issue_comments: Vec<IssueComment>,
    pub reviews: Vec<ReviewSummary>,
    pub review_threads: Vec<ReviewThread>,
}

/// PR データを API から一括再取得する（キャッシュをスキップして最新データを取得）
pub async fn reload_pr_data(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<ReloadedData> {
    // コミット一覧と PR 情報を並列取得
    let (commits, pr) = tokio::try_join!(
        github::commits::fetch_commits(client, owner, repo, pr_number),
        github::pr::fetch_pr(client, owner, repo, pr_number),
    )?;
    let metadata = extract_pr_metadata(&pr);
    let head_sha = commits.last().map(|c| c.sha.as_str()).unwrap_or("");

    // review threads を別スレッドで取得（GraphQL CLI 呼び出しのため spawn_blocking）
    let threads_handle = {
        let owner = owner.to_string();
        let repo = repo.to_string();
        tokio::task::spawn_blocking(move || {
            github::comments::fetch_review_threads(&owner, &repo, pr_number).unwrap_or_default()
        })
    };

    // ファイル取得とレビューコメント・Issue コメント・Reviews を並列実行
    let data_future = fetch_all(client, owner, repo, &commits, true);
    let comments_future = github::comments::fetch_review_comments(client, owner, repo, pr_number);
    let issue_comments_future =
        github::comments::fetch_issue_comments(client, owner, repo, pr_number);
    let reviews_future = github::review::fetch_reviews(client, owner, repo, pr_number);

    let (files_map, review_comments, issue_comments, reviews) = tokio::try_join!(
        data_future,
        comments_future,
        issue_comments_future,
        reviews_future,
    )?;

    let review_threads = threads_handle.await.unwrap_or_default();

    // 新しいキャッシュを書き込み
    github::cache::write_cache(
        owner,
        repo,
        pr_number,
        &github::cache::PrCache {
            version: github::cache::CACHE_VERSION,
            head_sha: head_sha.to_string(),
            files_map: files_map.clone(),
            review_threads: review_threads.clone(),
        },
    );

    Ok(ReloadedData {
        metadata,
        commits,
        files_map,
        review_comments,
        issue_comments,
        reviews,
        review_threads,
    })
}

#[tokio::main]
async fn main() {
    let _ = color_eyre::install();
    if let Err(e) = run().await {
        // エラーチェーンから根本原因メッセージを抽出してユーザーフレンドリーに表示
        let root = e.root_cause().to_string();
        let message = if root.contains("Not Found") {
            "PR or repository not found. Check the PR number and repository name.".to_string()
        } else if root.contains("rate limit") {
            "GitHub API rate limit exceeded. Please try again later.".to_string()
        } else if root.contains("401") || root.contains("Bad credentials") {
            "Authentication failed. Run `gh auth login` to authenticate.".to_string()
        } else {
            format!("{e:#}")
        };
        eprintln!("Error: {message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    use app::LoadPhase;
    use tokio::sync::mpsc;

    let cli = Cli::parse();

    // リポジトリ情報を解決
    let (owner, repo) = resolve_repo(&cli.repo)?;

    let current_user = fetch_current_user();

    // GitHub APIクライアントを作成
    let client = github::client::create_client()?;
    eprintln!("Fetching PR #{}...", cli.pr_number);

    // ── Phase A: ブロッキング ──
    // コミット一覧とPR情報を常にAPI取得
    // （HEAD SHA判定 + キャッシュヒット時もPR状態の最新性を保証するため）
    let (commits, pr) = tokio::try_join!(
        github::commits::fetch_commits(&client, &owner, &repo, cli.pr_number),
        github::pr::fetch_pr(&client, &owner, &repo, cli.pr_number),
    )?;
    let metadata = extract_pr_metadata(&pr);
    let head_sha = commits.last().map(|c| c.sha.clone()).unwrap_or_default();

    // キャッシュ判定
    let (files_map, cached_review_threads, cache_hit) = if !cli.no_cache {
        if let Some(cached) = github::cache::read_cache(&owner, &repo, cli.pr_number) {
            if cached.head_sha == head_sha {
                eprintln!(
                    "Using cached data (HEAD: {})",
                    &head_sha[..SHORT_SHA_LEN.min(head_sha.len())]
                );
                (cached.files_map, cached.review_threads, true)
            } else {
                eprintln!(
                    "Cache stale (expected {}, got {})",
                    &cached.head_sha[..SHORT_SHA_LEN.min(cached.head_sha.len())],
                    &head_sha[..SHORT_SHA_LEN.min(head_sha.len())]
                );
                (HashMap::new(), Vec::new(), false)
            }
        } else {
            eprintln!("No cache found, fetching from API...");
            (HashMap::new(), Vec::new(), false)
        }
    } else {
        eprintln!("Cache disabled, fetching from API...");
        (HashMap::new(), Vec::new(), false)
    };

    // テーマ検出（ratatui::init() の前に実行 — raw mode では OSC クエリが動かない）
    let theme = if cli.light {
        ThemeMode::Light
    } else if cli.dark {
        ThemeMode::Dark
    } else {
        detect_theme()
    };

    // 画像プロトコル検出（ratatui::init() の前に実行 — raw mode では OSC クエリが動かない）
    let picker = ratatui_image::picker::Picker::from_query_stdio().ok();

    let is_own_pr = !current_user.is_empty() && current_user == metadata.pr_author;

    // ── チャネル作成 ──
    let (tx, rx) = mpsc::unbounded_channel::<AsyncData>();

    // ── Phase B: バックグラウンド非同期タスク ──
    // ロード状態の初期化
    let loading = app::LoadingState {
        files: if cache_hit {
            LoadPhase::Done
        } else {
            LoadPhase::Loading
        },
        conversation: LoadPhase::Loading,
        media: LoadPhase::Loading,
    };

    // B1: Conversation データ（4 API を try_join! → ConversationData 送信）
    {
        let tx = tx.clone();
        let client = client.clone();
        let owner = owner.clone();
        let repo = repo.clone();
        let pr_number = cli.pr_number;
        tokio::spawn(async move {
            let threads_handle = {
                let owner = owner.clone();
                let repo = repo.clone();
                tokio::task::spawn_blocking(move || {
                    github::comments::fetch_review_threads(&owner, &repo, pr_number)
                        .unwrap_or_default()
                })
            };

            let result = tokio::try_join!(
                github::comments::fetch_review_comments(&client, &owner, &repo, pr_number),
                github::comments::fetch_issue_comments(&client, &owner, &repo, pr_number),
                github::review::fetch_reviews(&client, &owner, &repo, pr_number),
            );

            match result {
                Ok((review_comments, issue_comments, reviews)) => {
                    let review_threads = threads_handle.await.unwrap_or_default();
                    let _ = tx.send(AsyncData::ConversationData {
                        review_comments,
                        issue_comments,
                        reviews,
                        review_threads,
                    });
                }
                Err(e) => {
                    let _ = tx.send(AsyncData::Error(
                        AsyncErrorKind::Conversation,
                        format!("Failed to load conversation: {e}"),
                    ));
                }
            }
        });
    }

    // B2: ファイル差分（キャッシュミス時のみ）
    if !cache_hit {
        let tx = tx.clone();
        let client = client.clone();
        let owner = owner.clone();
        let repo = repo.clone();
        let commits = commits.clone();
        tokio::spawn(async move {
            match fetch_all(&client, &owner, &repo, &commits, true).await {
                Ok(files_map) => {
                    let _ = tx.send(AsyncData::FilesMap(files_map));
                }
                Err(e) => {
                    let _ = tx.send(AsyncData::Error(
                        AsyncErrorKind::Files,
                        format!("Failed to load files: {e}"),
                    ));
                }
            }
        });
    }

    // B3: 画像（PR body からURL収集 → ダウンロード）
    {
        let tx = tx.clone();
        let pr_body = metadata.pr_body.clone();
        tokio::spawn(async move {
            let image_urls = app::collect_image_urls(&pr_body);
            let media_cache = if image_urls.is_empty() {
                github::media::MediaCache::new()
            } else {
                github::media::download_media(image_urls).await
            };
            let _ = tx.send(AsyncData::MediaData(media_cache));
        });
    }

    // sender を全 spawn に clone 済みなので元の tx を drop
    drop(tx);

    // ── TUI 起動 ──
    let terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;

    let mut app = App::new(
        cli.pr_number,
        format!("{}/{}", owner, repo),
        metadata.pr_title,
        metadata.pr_body,
        metadata.pr_author,
        metadata.pr_base_branch,
        metadata.pr_head_branch,
        metadata.pr_created_at,
        metadata.pr_state,
        commits,
        files_map,
        Vec::new(), // review_comments: Phase B で到着
        Vec::new(), // conversation: Phase B で到着
        Some(client),
        theme,
        is_own_pr,
        current_user,
        cached_review_threads,
        Some(rx),
        loading,
        head_sha,
        cache_hit, // キャッシュヒット = 既に書き込み済み → 再書き込みスキップ
    );
    app.set_media(picker, MediaCache::new());
    let result = app.run(terminal);

    crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use github::comments::{ReviewComment, ReviewCommentUser};

    fn make_review_comment(
        id: u64,
        body: &str,
        path: &str,
        line: Option<usize>,
        in_reply_to_id: Option<u64>,
        created_at: &str,
    ) -> ReviewComment {
        ReviewComment {
            id,
            body: body.to_string(),
            path: path.to_string(),
            line,
            start_line: None,
            side: None,
            start_side: None,
            commit_id: "abc123".to_string(),
            user: ReviewCommentUser {
                login: "user1".to_string(),
            },
            created_at: created_at.to_string(),
            in_reply_to_id,
        }
    }

    #[test]
    fn test_build_conversation_thread_grouping() {
        let root = make_review_comment(
            1,
            "root comment",
            "src/main.rs",
            Some(10),
            None,
            "2024-01-01T00:00:00Z",
        );
        let reply1 = make_review_comment(
            2,
            "reply 1",
            "src/main.rs",
            Some(10),
            Some(1),
            "2024-01-01T01:00:00Z",
        );
        let reply2 = make_review_comment(
            3,
            "reply 2",
            "src/main.rs",
            Some(10),
            Some(1),
            "2024-01-01T02:00:00Z",
        );

        let entries = build_conversation(vec![], vec![], vec![root, reply1, reply2], &[]);
        assert_eq!(entries.len(), 1);

        match &entries[0].kind {
            ConversationKind::CodeComment {
                path,
                line,
                replies,
                ..
            } => {
                assert_eq!(path, "src/main.rs");
                assert_eq!(*line, Some(10));
                assert_eq!(replies.len(), 2);
                assert_eq!(replies[0].body, "reply 1");
                assert_eq!(replies[1].body, "reply 2");
            }
            _ => panic!("Expected CodeComment"),
        }
    }

    #[test]
    fn test_build_conversation_chronological_sort() {
        let issue = IssueComment {
            id: 100,
            body: Some("issue comment".to_string()),
            user: ReviewCommentUser {
                login: "user1".to_string(),
            },
            created_at: "2024-01-01T02:00:00Z".to_string(),
        };
        let code = make_review_comment(
            1,
            "code comment",
            "src/lib.rs",
            Some(5),
            None,
            "2024-01-01T01:00:00Z",
        );

        let entries = build_conversation(vec![issue], vec![], vec![code], &[]);
        assert_eq!(entries.len(), 2);

        // code comment (01:00) は issue comment (02:00) より前に来る
        assert!(matches!(
            entries[0].kind,
            ConversationKind::CodeComment { .. }
        ));
        assert!(matches!(entries[1].kind, ConversationKind::IssueComment));
    }

    #[test]
    fn test_build_conversation_with_resolved_thread() {
        let root = make_review_comment(
            1,
            "resolved comment",
            "src/main.rs",
            Some(10),
            None,
            "2024-01-01T00:00:00Z",
        );
        let threads = vec![ReviewThread {
            node_id: "RT_abc".to_string(),
            is_resolved: true,
            root_comment_database_id: 1,
        }];

        let entries = build_conversation(vec![], vec![], vec![root], &threads);
        assert_eq!(entries.len(), 1);

        match &entries[0].kind {
            ConversationKind::CodeComment {
                is_resolved,
                thread_node_id,
                ..
            } => {
                assert!(*is_resolved);
                assert_eq!(thread_node_id.as_deref(), Some("RT_abc"));
            }
            _ => panic!("Expected CodeComment"),
        }
    }

    #[test]
    fn test_build_conversation_unresolved_without_thread_info() {
        let root = make_review_comment(
            99,
            "no thread info",
            "src/lib.rs",
            Some(5),
            None,
            "2024-01-01T00:00:00Z",
        );

        // スレッド情報なし → is_resolved: false, thread_node_id: None
        let entries = build_conversation(vec![], vec![], vec![root], &[]);
        assert_eq!(entries.len(), 1);

        match &entries[0].kind {
            ConversationKind::CodeComment {
                is_resolved,
                thread_node_id,
                ..
            } => {
                assert!(!*is_resolved);
                assert!(thread_node_id.is_none());
            }
            _ => panic!("Expected CodeComment"),
        }
    }
}
