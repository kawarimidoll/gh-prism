mod app;
mod demo;
mod git;
mod github;

use app::{
    App, CodeCommentReply, ConversationEntry, ConversationKind, MergeableStatus, ReviewVerdict,
    ThemeMode,
};
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
    pub pr_state: app::PrState,
    pub mergeable_state: Option<MergeableStatus>,
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
            app::PrState::Merged
        } else {
            match pr.state {
                Some(octocrab::models::IssueState::Open) => app::PrState::Open,
                _ => app::PrState::Closed,
            }
        },
        mergeable_state: pr.mergeable_state.as_ref().map(|s| {
            use octocrab::models::pulls::MergeableState;
            match s {
                MergeableState::Clean | MergeableState::HasHooks => MergeableStatus::Clean,
                MergeableState::Unstable => MergeableStatus::Unstable,
                MergeableState::Behind => MergeableStatus::Behind,
                MergeableState::Blocked => MergeableStatus::Blocked,
                MergeableState::Dirty => MergeableStatus::Dirty,
                MergeableState::Draft => MergeableStatus::Draft,
                MergeableState::Unknown => MergeableStatus::Unknown,
                _ => MergeableStatus::Unknown,
            }
        }),
    }
}

/// 非同期エラーの発生元
pub enum AsyncErrorKind {
    Files,
    Conversation,
    Media,
}

/// 楽観的更新の操作ID
#[derive(Debug, Clone)]
pub enum OpId {
    SubmitReview,
    Merge,
    CloseToggle,
    IssueComment,
    ReplyComment,
    ResolveToggle,
    AddReaction,
    Reload,
}

/// 楽観的更新の結果ペイロード
pub enum OpPayload {
    None,
    IssueComment(IssueComment),
    ReplyComment(ReviewComment),
    Reload(Box<ReloadedData>),
}

/// バックグラウンド非同期タスクから App に送信するデータ
pub enum AsyncData {
    FilesMap(HashMap<String, Vec<DiffFile>>),
    ConversationData {
        review_comments: Vec<ReviewComment>,
        issue_comments: Vec<IssueComment>,
        reviews: Vec<ReviewSummary>,
        review_threads: Vec<ReviewThread>,
        /// comment_id → (content → reaction_id) の per-user リアクション情報
        user_reactions: HashMap<u64, HashMap<String, u64>>,
    },
    MediaData(MediaCache),
    Error(AsyncErrorKind, String),
    OpSuccess(OpId, OpPayload),
    OpFailure(OpId, String),
}

pub(crate) const VERSION: &str = match option_env!("GH_PRISM_VERSION") {
    Some(v) => v,
    None => env!("DEV_VERSION"),
};

#[derive(Parser)]
#[command(name = "prism", version = VERSION)]
#[command(about = "A TUI tool for reviewing GitHub Pull Requests")]
struct Cli {
    /// Pull Request number (required unless --demo)
    pr_number: Option<u64>,

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

    /// Launch in demo mode with mock data (no API calls)
    #[arg(long)]
    demo: bool,
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

/// リポジトリの権限と許可された merge 方式を一括取得する
pub struct RepoPermissions {
    pub can_merge: bool,
    pub allowed_merge_methods: Vec<app::MergeMethod>,
}

pub fn fetch_repo_permissions(owner: &str, repo: &str) -> RepoPermissions {
    let output = std::process::Command::new("gh")
        .args([
            "api",
            &format!("repos/{owner}/{repo}"),
            "-q",
            r#"[.permissions.push, .allow_merge_commit, .allow_squash_merge, .allow_rebase_merge] | @tsv"#,
        ])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();

    let fields: Vec<&str> = output.trim().split('\t').collect();
    let can_merge = fields.first().is_some_and(|s| *s == "true");
    let allow_merge = fields.get(1).is_some_and(|s| *s == "true");
    let allow_squash = fields.get(2).is_some_and(|s| *s == "true");
    let allow_rebase = fields.get(3).is_some_and(|s| *s == "true");

    let mut methods = Vec::new();
    if allow_merge {
        methods.push(app::MergeMethod::Merge);
    }
    if allow_squash {
        methods.push(app::MergeMethod::Squash);
    }
    if allow_rebase {
        methods.push(app::MergeMethod::Rebase);
    }
    // API が全て false を返した場合のフォールバック
    if methods.is_empty() {
        methods = app::MergeMethod::ALL.to_vec();
    }

    RepoPermissions {
        can_merge,
        allowed_merge_methods: methods,
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

/// リアクションがあるコメントの per-user リアクション情報を一括フェッチ
async fn fetch_user_reactions(
    client: &Octocrab,
    owner: &str,
    repo: &str,
    issue_comments: &[IssueComment],
    review_comments: &[ReviewComment],
    username: &str,
) -> HashMap<u64, HashMap<String, u64>> {
    use futures::stream::{FuturesUnordered, StreamExt};

    if username.is_empty() {
        return HashMap::new();
    }

    use std::pin::Pin;
    type ReactionFut = Pin<Box<dyn Future<Output = (u64, HashMap<String, u64>)> + Send>>;
    let mut futures: FuturesUnordered<ReactionFut> = FuturesUnordered::new();

    // リアクションがある issue comment のみフェッチ
    for c in issue_comments {
        if c.reactions.as_ref().is_some_and(|r| !r.is_empty()) {
            let client = client.clone();
            let owner = owner.to_string();
            let repo = repo.to_string();
            let username = username.to_string();
            let comment_id = c.id;
            futures.push(Box::pin(async move {
                let result = github::comments::fetch_user_reactions_for_issue_comment(
                    &client, &owner, &repo, comment_id, &username,
                )
                .await;
                (comment_id, result.unwrap_or_default())
            }));
        }
    }

    // リアクションがある review comment のルートコメントのみフェッチ
    for rc in review_comments {
        if rc.in_reply_to_id.is_none() && rc.reactions.as_ref().is_some_and(|r| !r.is_empty()) {
            let client = client.clone();
            let owner = owner.to_string();
            let repo = repo.to_string();
            let username = username.to_string();
            let comment_id = rc.id;
            futures.push(Box::pin(async move {
                let result = github::comments::fetch_user_reactions_for_review_comment(
                    &client, &owner, &repo, comment_id, &username,
                )
                .await;
                (comment_id, result.unwrap_or_default())
            }));
        }
    }

    let mut map = HashMap::new();
    while let Some((comment_id, user_reactions)) = futures.next().await {
        if !user_reactions.is_empty() {
            map.insert(comment_id, user_reactions);
        }
    }
    map
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
    // review_id → submitted_at のルックアップマップ（CodeComment のソートキーに使用）
    let review_time_map: HashMap<u64, &str> = reviews
        .iter()
        .filter_map(|r| r.submitted_at.as_deref().map(|t| (r.id, t)))
        .collect();

    let mut entries = Vec::new();

    for c in issue_comments {
        entries.push(ConversationEntry {
            author: c.user.login,
            body: c.body.unwrap_or_default(),
            created_at: c.created_at,
            kind: ConversationKind::IssueComment { comment_id: c.id },
            reactions: c.reactions,
            user_reaction_ids: HashMap::new(),
        });
    }

    for r in &reviews {
        // submitted_at が None のレビューは未送信（下書き）なのでスキップ
        let Some(submitted_at) = r.submitted_at.as_deref() else {
            continue;
        };
        let body = r.body.as_deref().unwrap_or("");
        // body 空かつ state が COMMENTED のみの review はスキップ（空コメントノイズ防止）
        if body.is_empty() && r.state == ReviewVerdict::Commented {
            continue;
        }
        entries.push(ConversationEntry {
            author: r.user.login.clone(),
            body: body.to_string(),
            created_at: submitted_at.to_string(),
            kind: ConversationKind::Review {
                state: r.state,
                node_id: r.node_id.clone(),
            },
            reactions: r.reactions.clone(),
            user_reaction_ids: HashMap::new(),
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
                    reactions: r.reactions.clone(),
                });
            }
        }

        let thread_info = thread_lookup.get(&root.id);
        // ソートキー: レビューの submitted_at を使い、GitHub Web と同じ表示順にする
        let sort_time = root
            .pull_request_review_id
            .and_then(|rid| review_time_map.get(&rid).copied())
            .unwrap_or(root.created_at.as_str());
        entries.push(ConversationEntry {
            author: root.user.login.clone(),
            body: root.body.clone(),
            created_at: sort_time.to_string(),
            kind: ConversationKind::CodeComment {
                path: root.path.clone(),
                line: root.line,
                replies,
                is_resolved: thread_info.is_some_and(|t| t.is_resolved),
                thread_node_id: thread_info.map(|t| t.node_id.clone()),
                root_comment_id: root.id,
            },
            reactions: root.reactions.clone(),
            user_reaction_ids: HashMap::new(),
        });
    }

    // created_at で時系列ソート（安定ソート: 同一時刻のエントリは push 順を維持。
    // Review → CodeComment の順で push されるため、GitHub Web と同じ並びになる）
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

    let (files_map, review_comments, issue_comments, mut reviews) = tokio::try_join!(
        data_future,
        comments_future,
        issue_comments_future,
        reviews_future,
    )?;

    // REST API は reviews にリアクションを含めないため GraphQL で補完
    github::review::populate_review_reactions(&mut reviews, owner, repo, pr_number);

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

    if cli.demo {
        return run_demo(cli).await;
    }

    let pr_number = cli.pr_number.ok_or_else(|| {
        color_eyre::eyre::eyre!("PR number is required (use --demo for demo mode)")
    })?;

    // リポジトリ情報を解決
    let (owner, repo) = resolve_repo(&cli.repo)?;

    let current_user = fetch_current_user();

    // GitHub APIクライアントを作成
    let client = github::client::create_client()?;
    eprintln!("Fetching PR #{}...", pr_number);

    // ── Phase A: ブロッキング ──
    // コミット一覧とPR情報を常にAPI取得
    // （HEAD SHA判定 + キャッシュヒット時もPR状態の最新性を保証するため）
    let (commits, pr) = tokio::try_join!(
        github::commits::fetch_commits(&client, &owner, &repo, pr_number),
        github::pr::fetch_pr(&client, &owner, &repo, pr_number),
    )?;
    let metadata = extract_pr_metadata(&pr);
    let head_sha = commits.last().map(|c| c.sha.clone()).unwrap_or_default();

    // キャッシュ判定
    let (files_map, cached_review_threads, cache_hit) = if !cli.no_cache {
        if let Some(cached) = github::cache::read_cache(&owner, &repo, pr_number) {
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
    let repo_perms = fetch_repo_permissions(&owner, &repo);

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

    // B1: Conversation データ（4 API を try_join! → per-user リアクション取得 → ConversationData 送信）
    {
        let tx = tx.clone();
        let client = client.clone();
        let owner = owner.clone();
        let repo = repo.clone();
        let current_user = current_user.clone();
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
                Ok((review_comments, issue_comments, mut reviews)) => {
                    let review_threads = threads_handle.await.unwrap_or_default();

                    // REST API は reviews にリアクションを含めないため GraphQL で補完
                    github::review::populate_review_reactions(
                        &mut reviews,
                        &owner,
                        &repo,
                        pr_number,
                    );

                    // リアクションがあるコメントの per-user リアクション情報を取得
                    let user_reactions = fetch_user_reactions(
                        &client,
                        &owner,
                        &repo,
                        &issue_comments,
                        &review_comments,
                        &current_user,
                    )
                    .await;

                    let _ = tx.send(AsyncData::ConversationData {
                        review_comments,
                        issue_comments,
                        reviews,
                        review_threads,
                        user_reactions,
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

    // api_tx 用に clone を保持してから元の tx を drop
    let api_tx = tx.clone();
    drop(tx);

    // ── TUI 起動 ──
    let terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;

    let mut app = App::new(
        pr_number,
        format!("{}/{}", owner, repo),
        metadata.pr_title,
        metadata.pr_body,
        metadata.pr_author,
        metadata.pr_base_branch,
        metadata.pr_head_branch,
        metadata.pr_created_at,
        metadata.pr_state,
        metadata.mergeable_state,
        commits,
        files_map,
        Vec::new(), // review_comments: Phase B で到着
        Vec::new(), // conversation: Phase B で到着
        Some(client),
        theme,
        is_own_pr,
        repo_perms.can_merge,
        repo_perms.allowed_merge_methods,
        current_user,
        cached_review_threads,
        Some(rx),
        loading,
        head_sha,
        cache_hit, // キャッシュヒット = 既に書き込み済み → 再書き込みスキップ
    );
    app.api_tx = Some(api_tx);
    app.set_media(picker, MediaCache::new());
    let result = app.run(terminal);

    crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
    ratatui::restore();
    result
}

async fn run_demo(cli: Cli) -> Result<()> {
    use app::LoadPhase;
    use tokio::sync::mpsc;

    // ── モックデータを同期構築 ──
    let metadata = demo::demo_metadata();
    let commits = demo::demo_commits();
    let head_sha = commits.last().map(|c| c.sha.clone()).unwrap_or_default();

    // テーマ検出（ターミナル依存なのでそのまま実行）
    let theme = if cli.light {
        ThemeMode::Light
    } else if cli.dark {
        ThemeMode::Dark
    } else {
        detect_theme()
    };

    // 画像プロトコル検出（raw mode 前に実行）
    let picker = ratatui_image::picker::Picker::from_query_stdio().ok();

    // ── チャネル作成 ──
    let (tx, rx) = mpsc::unbounded_channel::<AsyncData>();

    // LoadingState を全 Loading で初期化
    let loading = app::LoadingState {
        files: LoadPhase::Loading,
        conversation: LoadPhase::Loading,
        media: LoadPhase::Loading,
    };

    // ── 0.5 秒 sleep 後にモックデータを送信（ローディング UI エミュレーション） ──
    let sleep_duration = std::time::Duration::from_millis(500);

    // B1: FilesMap
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(sleep_duration).await;
            let _ = tx.send(AsyncData::FilesMap(demo::demo_files_map()));
        });
    }

    // B2: ConversationData
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(sleep_duration).await;
            let _ = tx.send(AsyncData::ConversationData {
                review_comments: demo::demo_review_comments(),
                issue_comments: demo::demo_issue_comments(),
                reviews: demo::demo_reviews(),
                review_threads: demo::demo_review_threads(),
                user_reactions: HashMap::new(),
            });
        });
    }

    // B3: MediaData（空の MediaCache）
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(sleep_duration).await;
            let _ = tx.send(AsyncData::MediaData(MediaCache::new()));
        });
    }

    let api_tx = tx.clone();
    drop(tx);

    // ── TUI 起動 ──
    let terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;

    let mut app = App::new(
        demo::DEMO_PR_NUMBER,
        demo::DEMO_REPO.to_string(),
        metadata.pr_title,
        metadata.pr_body,
        metadata.pr_author,
        metadata.pr_base_branch,
        metadata.pr_head_branch,
        metadata.pr_created_at,
        metadata.pr_state,
        metadata.mergeable_state,
        commits,
        HashMap::new(), // files_map: Phase B で到着
        Vec::new(),     // review_comments: Phase B で到着
        Vec::new(),     // conversation: Phase B で到着
        None,           // client: None で書き込み操作を無効化
        theme,
        false,                          // is_own_pr: false でレビュー UI を表示可能に
        true,                           // can_merge: true でデモ時も merge 操作を体験可能に
        app::MergeMethod::ALL.to_vec(), // allowed_merge_methods: 全方式
        demo::DEMO_CURRENT_USER.to_string(),
        Vec::new(), // review_threads: Phase B で到着
        Some(rx),
        loading,
        head_sha,
        true, // cache_written: true でキャッシュ書き込みをスキップ
    );
    app.demo_mode = true;
    app.api_tx = Some(api_tx);
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
        pull_request_review_id: Option<u64>,
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
            pull_request_review_id,
            reactions: None,
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
            None,
        );
        let reply1 = make_review_comment(
            2,
            "reply 1",
            "src/main.rs",
            Some(10),
            Some(1),
            "2024-01-01T01:00:00Z",
            None,
        );
        let reply2 = make_review_comment(
            3,
            "reply 2",
            "src/main.rs",
            Some(10),
            Some(1),
            "2024-01-01T02:00:00Z",
            None,
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
            reactions: None,
        };
        let code = make_review_comment(
            1,
            "code comment",
            "src/lib.rs",
            Some(5),
            None,
            "2024-01-01T01:00:00Z",
            None,
        );

        let entries = build_conversation(vec![issue], vec![], vec![code], &[]);
        assert_eq!(entries.len(), 2);

        // code comment (01:00) は issue comment (02:00) より前に来る
        assert!(matches!(
            entries[0].kind,
            ConversationKind::CodeComment { .. }
        ));
        assert!(matches!(
            entries[1].kind,
            ConversationKind::IssueComment { .. }
        ));
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
            None,
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
            None,
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

    #[test]
    fn test_build_conversation_code_comment_sorted_by_review_submitted_at() {
        use github::review::ReviewSummary;

        // CodeComment の created_at は 01:00 だが、所属レビューの submitted_at は 03:00
        let code = make_review_comment(
            1,
            "code comment",
            "src/lib.rs",
            Some(5),
            None,
            "2024-01-01T01:00:00Z",
            Some(1000), // review id
        );

        // IssueComment の created_at は 02:00
        let issue = IssueComment {
            id: 100,
            body: Some("issue comment".to_string()),
            user: ReviewCommentUser {
                login: "user1".to_string(),
            },
            created_at: "2024-01-01T02:00:00Z".to_string(),
            reactions: None,
        };

        // Review id=1000 の submitted_at は 03:00
        let review = ReviewSummary {
            id: 1000,
            node_id: "PRR_test_001".to_string(),
            user: ReviewCommentUser {
                login: "reviewer".to_string(),
            },
            body: Some("looks good".to_string()),
            state: ReviewVerdict::Approved,
            submitted_at: Some("2024-01-01T03:00:00Z".to_string()),
            reactions: None,
        };

        let entries = build_conversation(vec![issue], vec![review], vec![code], &[]);
        // Review(body あり) + IssueComment + CodeComment = 3 エントリ
        assert_eq!(entries.len(), 3);

        // IssueComment (02:00) → Review (03:00) → CodeComment (review の 03:00 で表示)
        assert!(matches!(
            entries[0].kind,
            ConversationKind::IssueComment { .. }
        ));
        assert!(matches!(entries[1].kind, ConversationKind::Review { .. }));
        assert!(matches!(
            entries[2].kind,
            ConversationKind::CodeComment { .. }
        ));
        // Review と CodeComment は同じ submitted_at だが、安定ソートにより push 順（Review が先）を維持
        assert_eq!(entries[1].created_at, "2024-01-01T03:00:00Z");
        assert_eq!(entries[2].created_at, "2024-01-01T03:00:00Z");
    }
}
