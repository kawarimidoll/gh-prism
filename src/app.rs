pub mod editor;
mod handler;
mod helpers;
mod markdown;
mod media;
mod navigation;
mod render;
mod types;

use helpers::{format_datetime, open_url_in_browser, truncate_path, truncate_str};
pub use media::{collect_image_urls, preprocess_pr_body};
pub use types::*;

use crate::github::comments::{self as comments, ReviewComment, ReviewThread};
use crate::github::commits::CommitInfo;
use crate::github::files::DiffFile;
use crate::github::media::MediaCache;
use crate::github::review::{self, PendingComment};
use color_eyre::Result;
use octocrab::Octocrab;
use ratatui::{
    DefaultTerminal,
    layout::Position,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::ListState,
};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::{HashMap, HashSet};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

pub struct App {
    should_quit: bool,
    focused_panel: Panel,
    mode: AppMode,
    pr_number: u64,
    repo: String,
    pr_title: String,
    pr_body: String,
    pr_author: String,
    pr_base_branch: String,
    pr_head_branch: String,
    pr_created_at: String,
    pr_state: String,
    commits: Vec<CommitInfo>,
    commit_list_state: ListState,
    files_map: HashMap<String, Vec<DiffFile>>,
    file_list_state: ListState,
    pr_desc_scroll: u16,
    /// PR Description ペインの表示可能行数（render 時に更新）
    pr_desc_view_height: u16,
    /// PR Description の Wrap 考慮済み視覚行数（render 時に更新）
    pr_desc_visual_total: u16,
    /// Commit Message ペインのスクロール位置
    commit_msg_scroll: u16,
    /// Commit Message ペインの表示可能行数（render 時に更新）
    commit_msg_view_height: u16,
    /// Commit Message の Wrap 考慮済み視覚行数（render 時に更新）
    commit_msg_visual_total: u16,
    /// Commit Overview ペインのスクロール位置
    commit_overview_scroll: u16,
    /// Commit Overview ペインの表示可能行数（render 時に更新）
    commit_overview_view_height: u16,
    /// Commit Overview の Wrap 考慮済み視覚行数（render 時に更新）
    commit_overview_visual_total: u16,
    /// DiffView パネルの表示状態
    pub diff: DiffViewState,
    /// 行選択モードでの選択状態
    line_selection: Option<LineSelection>,
    /// レビュー・コメント関連の状態
    pub review: ReviewState,
    /// GitHub API クライアント（テスト時は None）
    client: Option<Octocrab>,
    /// ステータスメッセージ（ヘッダーバーに表示、3秒後に自動クリア）
    status_message: Option<StatusMessage>,
    /// 2キーシーケンスの1文字目（`]` or `[`）を保持
    pending_key: Option<char>,
    /// ヘルプ画面のスクロール位置
    help_scroll: u16,
    /// ヘルプ画面のコンテキスト（`?` 押下時のフォーカスパネルで上書きされる。初期値は未使用）
    help_context_panel: Panel,
    /// Zoom モード（フォーカスペインのみ全画面表示）
    zoomed: bool,
    /// viewed 済みファイルのマップ（コミット SHA → ファイル名の Set）
    viewed_files: HashMap<String, HashSet<String>>,
    /// PR Description のマークダウンレンダリングキャッシュ
    pr_desc_rendered: Option<Text<'static>>,
    /// Conversation ペインのマークダウンレンダリングキャッシュ
    conversation_rendered: Option<Vec<Line<'static>>>,
    /// カラーテーマ（ライト/ダーク）
    theme: ThemeMode,
    /// 各ペインの描画領域キャッシュ（マウスヒットテスト用、render 時に更新）
    pub layout: LayoutCache,
    /// PR body 中のメディア参照
    media_refs: Vec<MediaRef>,
    /// 画像プロトコル検出結果（None = 画像表示不可）
    picker: Option<Picker>,
    /// ダウンロード済み画像キャッシュ
    media_cache: MediaCache,
    /// メディアビューアの現在のインデックス
    media_viewer_index: usize,
    /// メディアビューアのプロトコルキャッシュ（URL → StatefulProtocol）
    media_protocol_cache: HashMap<String, StatefulProtocol>,
    /// バックグラウンドでプロトコル生成中のワーカー
    media_protocol_worker: Option<std::thread::JoinHandle<(String, StatefulProtocol)>>,
    /// (commit_sha, filename) → 可視レビューコメント数のキャッシュ（起動時に計算）
    visible_review_comment_cache: HashMap<(String, String), usize>,
    /// 自分のPRかどうか（Approve/Request Changesを非表示にする）
    is_own_pr: bool,
    /// 現在の認証ユーザー名（リロード時の is_own_pr 再判定に使用）
    current_user: String,
    /// Conversation エントリ（Issue Comment + Review を時系列マージ）
    conversation: Vec<ConversationEntry>,
    /// Conversation ペインのスクロール位置
    conversation_scroll: u16,
    /// Conversation ペインの表示可能行数（render 時に更新）
    conversation_view_height: u16,
    /// Conversation の Wrap 考慮済み視覚行数（render 時に更新）
    conversation_visual_total: u16,
    /// Issue Comment 送信フラグ（draw 後に実行）
    needs_issue_comment_submit: bool,
    /// Reply Comment 送信フラグ（draw 後に実行）
    needs_reply_submit: bool,
    /// PR データリロードフラグ（draw 後に実行）
    needs_reload: bool,
    /// バックグラウンド非同期データ受信チャネル
    async_rx: Option<mpsc::UnboundedReceiver<crate::AsyncData>>,
    /// 非同期データのロード状態
    pub loading: LoadingState,
    /// HEAD SHA（キャッシュ書き込み用）
    head_sha: String,
    /// キャッシュ書き込み済みフラグ
    cache_written: bool,
    /// Conversation ペインのエントリカーソル位置
    conversation_cursor: usize,
    /// Conversation エントリごとの論理行オフセット（ensure_conversation_rendered で計算）
    conversation_entry_offsets: Vec<usize>,
    /// Conversation エントリごとの Wrap 考慮済み視覚行オフセット（render 時に計算、navigation で参照）
    conversation_visual_offsets: Vec<u16>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pr_number: u64,
        repo: String,
        pr_title: String,
        pr_body: String,
        pr_author: String,
        pr_base_branch: String,
        pr_head_branch: String,
        pr_created_at: String,
        pr_state: String,
        commits: Vec<CommitInfo>,
        files_map: HashMap<String, Vec<DiffFile>>,
        review_comments: Vec<ReviewComment>,
        conversation: Vec<ConversationEntry>,
        client: Option<Octocrab>,
        theme: ThemeMode,
        is_own_pr: bool,
        current_user: String,
        review_threads: Vec<ReviewThread>,
        async_rx: Option<mpsc::UnboundedReceiver<crate::AsyncData>>,
        loading: LoadingState,
        head_sha: String,
        cache_written: bool,
    ) -> Self {
        let mut commit_list_state = ListState::default();
        if !commits.is_empty() {
            commit_list_state.select(Some(0));
        }

        // root_comment_database_id → ReviewThread のマップを構築
        let thread_map: HashMap<u64, ReviewThread> = review_threads
            .into_iter()
            .map(|t| (t.root_comment_database_id, t))
            .collect();

        // (commit_sha, filename) → 可視レビューコメント数を事前計算
        let visible_review_comment_cache =
            Self::build_visible_comment_cache(&review_comments, &files_map);

        // 最初のコミットのファイル数に基づいて file_list_state を初期化
        let mut file_list_state = ListState::default();
        if let Some(first_commit) = commits.first()
            && let Some(files) = files_map.get(&first_commit.sha)
            && !files.is_empty()
        {
            file_list_state.select(Some(0));
        }

        Self {
            should_quit: false,
            focused_panel: Panel::PrDescription,
            mode: AppMode::default(),
            pr_number,
            repo,
            pr_title,
            pr_body,
            pr_author,
            pr_base_branch,
            pr_head_branch,
            pr_created_at,
            pr_state,
            commits,
            commit_list_state,
            files_map,
            file_list_state,
            pr_desc_scroll: 0,
            pr_desc_view_height: 10, // 初期値、render で更新される
            pr_desc_visual_total: 0, // 初期値、render で更新される
            commit_msg_scroll: 0,
            commit_msg_view_height: 4,  // 初期値、render で更新される
            commit_msg_visual_total: 0, // 初期値、render で更新される
            commit_overview_scroll: 0,
            commit_overview_view_height: 10, // 初期値、render で更新される
            commit_overview_visual_total: 0, // 初期値、render で更新される
            diff: DiffViewState::default(),
            line_selection: None,
            review: ReviewState {
                review_comments,
                thread_map,
                ..Default::default()
            },
            client,
            status_message: None,
            pending_key: None,
            help_scroll: 0,
            help_context_panel: Panel::PrDescription,
            zoomed: false,
            viewed_files: HashMap::new(),
            pr_desc_rendered: None,
            conversation_rendered: None,
            theme,
            layout: LayoutCache::default(),
            media_refs: Vec::new(),
            picker: None,
            media_cache: MediaCache::new(),
            media_viewer_index: 0,
            media_protocol_cache: HashMap::new(),
            media_protocol_worker: None,
            visible_review_comment_cache,
            is_own_pr,
            current_user,
            conversation,
            conversation_scroll: 0,
            conversation_view_height: 10, // 初期値、render で更新される
            conversation_visual_total: 0, // 初期値、render で更新される
            needs_issue_comment_submit: false,
            needs_reply_submit: false,
            needs_reload: false,
            async_rx,
            loading,
            head_sha,
            cache_written,
            conversation_cursor: 0,
            conversation_entry_offsets: Vec::new(),
            conversation_visual_offsets: Vec::new(),
        }
    }

    /// 選択可能なレビューイベントを返す（自分のPRではCommentのみ）
    fn available_events(&self) -> &[ReviewEvent] {
        if self.is_own_pr {
            &ReviewEvent::ALL[..1]
        } else {
            &ReviewEvent::ALL
        }
    }

    /// 画像プロトコル検出結果と画像キャッシュをセットする
    pub fn set_media(&mut self, picker: Option<Picker>, media_cache: MediaCache) {
        self.picker = picker;
        self.media_cache = media_cache;
    }

    /// PR body 内のメディア参照の数を返す（画像 + 動画）
    fn media_count(&self) -> usize {
        self.media_refs.len()
    }

    /// PR body 内の N 番目のメディア参照を返す
    fn media_ref_at(&self, index: usize) -> Option<&MediaRef> {
        self.media_refs.get(index)
    }

    /// 現在選択中のコミットのファイル一覧を取得
    fn current_files(&self) -> &[DiffFile] {
        if let Some(idx) = self.commit_list_state.selected()
            && let Some(commit) = self.commits.get(idx)
            && let Some(files) = self.files_map.get(&commit.sha)
        {
            return files;
        }
        &[]
    }

    /// ファイル選択をリセット（最初のファイルを選択、またはNone）
    fn reset_file_selection(&mut self) {
        let has_files = !self.current_files().is_empty();
        if has_files {
            self.file_list_state.select(Some(0));
        } else {
            self.file_list_state.select(None);
        }
        self.diff.cursor_line = 0;
        self.diff.scroll = 0;
        self.commit_msg_scroll = 0;
        self.commit_overview_scroll = 0;
        // 先頭の @@ 行をスキップ
        let max = self.current_diff_line_count();
        self.diff.cursor_line = self.skip_hunk_header_forward(0, max);
    }

    /// 現在選択中のファイルを取得
    fn current_file(&self) -> Option<&DiffFile> {
        let files = self.current_files();
        if let Some(idx) = self.file_list_state.selected() {
            return files.get(idx);
        }
        None
    }

    /// ファイルが viewed か判定
    fn is_file_viewed(&self, sha: &str, filename: &str) -> bool {
        self.viewed_files
            .get(sha)
            .is_some_and(|files| files.contains(filename))
    }

    /// viewed フラグをトグル（FileTree 用）
    fn toggle_viewed(&mut self) {
        let Some(sha) = self.current_commit_sha() else {
            return;
        };
        if let Some(file) = self.current_file() {
            let name = file.filename.clone();
            let set = self.viewed_files.entry(sha).or_default();
            if !set.remove(&name) {
                set.insert(name);
            }
        }
    }

    /// コミットの全ファイルが viewed か判定（導出状態）
    fn is_commit_viewed(&self, sha: &str) -> bool {
        if let Some(files) = self.files_map.get(sha) {
            !files.is_empty() && files.iter().all(|f| self.is_file_viewed(sha, &f.filename))
        } else {
            false
        }
    }

    /// viewed コミット数を返す
    fn viewed_commit_count(&self) -> usize {
        self.commits
            .iter()
            .filter(|c| self.is_commit_viewed(&c.sha))
            .count()
    }

    /// 現在選択中のコミット SHA を返す
    fn current_commit_sha(&self) -> Option<String> {
        self.commit_list_state
            .selected()
            .and_then(|idx| self.commits.get(idx))
            .map(|c| c.sha.clone())
    }

    /// CommitList で viewed トグル（全ファイル一括）
    fn toggle_commit_viewed(&mut self) {
        let Some(sha) = self.current_commit_sha() else {
            return;
        };
        let Some(files) = self.files_map.get(&sha) else {
            return;
        };
        let filenames: Vec<String> = files.iter().map(|f| f.filename.clone()).collect();
        if self.is_commit_viewed(&sha) {
            // 全ファイルを unview
            if let Some(set) = self.viewed_files.get_mut(&sha) {
                for name in &filenames {
                    set.remove(name);
                }
            }
        } else {
            // 全ファイルを view
            let set = self.viewed_files.entry(sha).or_default();
            for name in filenames {
                set.insert(name);
            }
        }
    }

    /// リスト選択行のハイライトスタイル（テーマ対応）
    fn highlight_style(&self) -> Style {
        match self.theme {
            ThemeMode::Dark => Style::default().bg(Color::DarkGray).fg(Color::White),
            ThemeMode::Light => Style::default().bg(Color::Indexed(254)).fg(Color::Black),
        }
    }

    /// Hunk ヘッダーのスタイル（テーマ対応）
    fn hunk_header_style(&self) -> Style {
        match self.theme {
            ThemeMode::Dark => Style::default().bg(Color::Indexed(238)).fg(Color::Cyan),
            ThemeMode::Light => Style::default().bg(Color::Indexed(252)).fg(Color::Cyan),
        }
    }

    /// テキストをシステムクリップボードにコピー
    fn copy_to_clipboard(&mut self, text: &str, label: &str) {
        let result = if cfg!(target_os = "macos") {
            std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(stdin) = child.stdin.as_mut() {
                        stdin.write_all(text.as_bytes())?;
                    }
                    child.wait()
                })
        } else {
            std::process::Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(stdin) = child.stdin.as_mut() {
                        stdin.write_all(text.as_bytes())?;
                    }
                    child.wait()
                })
        };

        match result {
            Ok(status) if status.success() => {
                self.status_message =
                    Some(StatusMessage::info(format!("✓ Copied {}: {}", label, text)));
            }
            _ => {
                self.status_message = Some(StatusMessage::error("✗ Failed to copy to clipboard"));
            }
        }
    }

    /// (commit_sha, filename) → 可視レビューコメント数のキャッシュを構築する
    fn build_visible_comment_cache(
        review_comments: &[ReviewComment],
        files_map: &HashMap<String, Vec<DiffFile>>,
    ) -> HashMap<(String, String), usize> {
        let mut cache = HashMap::new();
        for (sha, files) in files_map {
            for f in files {
                let Some(patch) = f.patch.as_deref() else {
                    continue;
                };
                let file_comments: Vec<&ReviewComment> = review_comments
                    .iter()
                    .filter(|c| c.path == f.filename && c.line.is_some())
                    .collect();
                if file_comments.is_empty() {
                    continue;
                }
                let line_map = review::parse_patch_line_map(patch);
                let mut line_set: HashSet<(usize, &str)> = HashSet::new();
                for info in line_map.iter().flatten() {
                    let side_str = match info.side {
                        review::Side::Left => "LEFT",
                        review::Side::Right => "RIGHT",
                    };
                    line_set.insert((info.file_line, side_str));
                }
                let count = file_comments
                    .iter()
                    .filter(|c| {
                        let line = c.line.unwrap();
                        let side = c.side.as_deref().unwrap_or("RIGHT");
                        line_set.contains(&(line, side))
                    })
                    .count();
                if count > 0 {
                    cache.insert((sha.clone(), f.filename.clone()), count);
                }
            }
        }
        cache
    }

    /// キャッシュから (commit_sha, filename) の可視レビューコメント数を取得
    fn cached_visible_comment_count(&self, commit_sha: &str, filename: &str) -> usize {
        self.visible_review_comment_cache
            .get(&(commit_sha.to_string(), filename.to_string()))
            .copied()
            .unwrap_or(0)
    }

    /// 現在のファイルの各 diff 行にある既存コメント数を返す（逆引きマッピング）
    fn existing_comment_counts(&self) -> HashMap<usize, usize> {
        let mut counts: HashMap<usize, usize> = HashMap::new();
        let Some(file) = self.current_file() else {
            return counts;
        };
        let Some(patch) = file.patch.as_deref() else {
            return counts;
        };

        // ファイルに該当するコメントを絞り込み（outdated な line=None は除外）
        let file_comments: Vec<&ReviewComment> = self
            .review
            .review_comments
            .iter()
            .filter(|c| c.path == file.filename && c.line.is_some())
            .collect();

        if file_comments.is_empty() {
            return counts;
        }

        // patch の逆引きマップ: (file_line, side) → diff_line_index
        let line_map = review::parse_patch_line_map(patch);
        let mut reverse: HashMap<(usize, &str), usize> = HashMap::new();
        for (idx, info) in line_map.iter().enumerate() {
            if let Some(info) = info {
                let side_str = match info.side {
                    review::Side::Left => "LEFT",
                    review::Side::Right => "RIGHT",
                };
                reverse.insert((info.file_line, side_str), idx);
            }
        }

        for comment in &file_comments {
            let line = comment.line.unwrap(); // filter で None は除外済み
            let side = comment.side.as_deref().unwrap_or("RIGHT");
            if let Some(&diff_idx) = reverse.get(&(line, side)) {
                *counts.entry(diff_idx).or_insert(0) += 1;
            }
        }

        counts
    }

    /// 指定 diff 行のコメントを取得（CommentView 用）
    fn comments_at_diff_line(&self, diff_line: usize) -> Vec<ReviewComment> {
        let Some(file) = self.current_file() else {
            return Vec::new();
        };
        let Some(patch) = file.patch.as_deref() else {
            return Vec::new();
        };

        let line_map = review::parse_patch_line_map(patch);
        let Some(Some(info)) = line_map.get(diff_line) else {
            return Vec::new();
        };

        let side_str = match info.side {
            review::Side::Left => "LEFT",
            review::Side::Right => "RIGHT",
        };

        self.review
            .review_comments
            .iter()
            .filter(|c| {
                c.path == file.filename
                    && c.line == Some(info.file_line)
                    && c.side.as_deref().unwrap_or("RIGHT") == side_str
            })
            .cloned()
            .collect()
    }

    pub fn run(&mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            // 期限切れのステータスメッセージを自動クリア
            if self.status_message.as_ref().is_some_and(|m| m.is_expired()) {
                self.status_message = None;
            }

            // バックグラウンドワーカーの完了チェック
            self.poll_media_protocol_worker();
            self.poll_async_data();

            terminal.draw(|frame| self.render(frame))?;

            // draw 後に submit を実行（ローディング表示を先にユーザーへ見せる）
            if let Some(event) = self.review.needs_submit.take() {
                self.submit_review_with_event(event);
                if self.review.quit_after_submit {
                    self.review.quit_after_submit = false;
                    self.should_quit = true;
                }
            }

            if self.needs_issue_comment_submit {
                self.needs_issue_comment_submit = false;
                self.submit_issue_comment();
            }

            if self.needs_reply_submit {
                self.needs_reply_submit = false;
                self.submit_reply_comment();
            }

            if self.needs_reload {
                self.needs_reload = false;
                self.execute_reload();
            }

            if self.review.needs_resolve_toggle.is_some() {
                self.execute_resolve_toggle();
            }

            self.handle_events()?;
        }
        Ok(())
    }

    /// PR Description のマークダウンレンダリングキャッシュを生成（未生成の場合のみ）
    fn ensure_pr_desc_rendered(&mut self) {
        if self.pr_desc_rendered.is_some() {
            return;
        }
        let (processed_body, media_refs) = preprocess_pr_body(&self.pr_body);
        self.media_refs = media_refs;

        // PR タイトルをヘッダー行として先頭に挿入（author は Info ペインに表示）
        let title_line = Line::styled(
            self.pr_title.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
        let separator = Line::from("──────────────");

        let text: Text<'static> = if processed_body.is_empty() {
            Text::from(vec![
                title_line,
                separator,
                Line::raw(""),
                Line::raw("(No description)"),
            ])
        } else {
            let mut lines: Vec<Line<'static>> = vec![title_line, separator, Line::raw("")];
            lines.extend(markdown::render_markdown(&processed_body, self.theme));
            Text::from(lines)
        };
        self.pr_desc_rendered = Some(text);
    }

    /// Conversation ペインのマークダウンレンダリングキャッシュを生成（未生成の場合のみ）
    fn ensure_conversation_rendered(&mut self) {
        if self.conversation_rendered.is_some() {
            return;
        }

        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut entry_offsets: Vec<usize> = Vec::new();

        if self.conversation.is_empty() {
            lines.push(Line::styled(
                " (No conversation)",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            for entry in &self.conversation {
                entry_offsets.push(lines.len());
                // ヘッダー行: @author (date) [STATE]
                let date_display = format_datetime(&entry.created_at);
                let mut header_spans = vec![
                    Span::styled(
                        format!(" @{}", entry.author),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!(" ({})", date_display),
                        Style::default().fg(Color::DarkGray),
                    ),
                ];

                // Review の場合は state ラベルを追加（COMMENTED は非表示）
                if let ConversationKind::Review { ref state } = entry.kind {
                    let label_opt = match state.as_str() {
                        "APPROVED" => Some(("APPROVED", Color::Green)),
                        "CHANGES_REQUESTED" => Some(("CHANGES REQUESTED", Color::Red)),
                        "DISMISSED" => Some(("DISMISSED", Color::DarkGray)),
                        _ => None, // COMMENTED やその他は非表示
                    };
                    if let Some((label, color)) = label_opt {
                        header_spans.push(Span::styled(
                            format!(" [{}]", label),
                            Style::default().fg(color),
                        ));
                    }
                }

                // CodeComment の場合はファイルパスと行番号を表示
                if let ConversationKind::CodeComment {
                    ref path,
                    line,
                    is_resolved,
                    ..
                } = entry.kind
                {
                    let location = if let Some(l) = line {
                        format!(" {}:{}", path, l)
                    } else {
                        format!(" {}", path)
                    };
                    header_spans.push(Span::styled(location, Style::default().fg(Color::Yellow)));
                    if is_resolved {
                        header_spans.push(Span::styled(
                            " [Resolved]",
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }

                lines.push(Line::from(header_spans));

                // 本文をマークダウンレンダリング（bat ハイライト or プレーンテキスト）
                if !entry.body.is_empty() {
                    lines.extend(markdown::render_markdown(&entry.body, self.theme));
                }

                // CodeComment のリプライを描画
                if let ConversationKind::CodeComment { ref replies, .. } = entry.kind {
                    for reply in replies {
                        let reply_date = format_datetime(&reply.created_at);
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("   @{}", reply.author),
                                Style::default().fg(Color::Cyan),
                            ),
                            Span::styled(
                                format!(" ({})", reply_date),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]));
                        if !reply.body.is_empty() {
                            // リプライ本文もマークダウンレンダリング
                            lines.extend(markdown::render_markdown(&reply.body, self.theme));
                        }
                    }
                }

                // 空行（エントリ間セパレータ）
                lines.push(Line::raw(""));
            }
            // 末尾のセンチネル（最後のエントリの終了位置）
            entry_offsets.push(lines.len());
        }

        self.conversation_entry_offsets = entry_offsets;
        // カーソル位置をクランプ
        if !self.conversation.is_empty() {
            self.conversation_cursor = self.conversation_cursor.min(self.conversation.len() - 1);
        }
        self.conversation_rendered = Some(lines);
    }

    /// PR Description の Wrap 考慮済み視覚行数を返す
    /// render 前は論理行数にフォールバック
    fn pr_desc_total_lines(&mut self) -> u16 {
        if self.pr_desc_visual_total > 0 {
            return self.pr_desc_visual_total;
        }
        // render 前のフォールバック（テスト等）
        self.ensure_pr_desc_rendered();
        self.pr_desc_rendered
            .as_ref()
            .map(|t| t.lines.len() as u16)
            .unwrap_or(0)
    }

    /// PR Description のスクロール上限を返す
    fn pr_desc_max_scroll(&mut self) -> u16 {
        self.pr_desc_total_lines()
            .saturating_sub(self.pr_desc_view_height)
    }

    /// PR Description のスクロール位置を上限にクランプする
    fn clamp_pr_desc_scroll(&mut self) {
        let max = self.pr_desc_max_scroll();
        if self.pr_desc_scroll > max {
            self.pr_desc_scroll = max;
        }
    }

    /// Conversation のスクロール上限を返す
    fn conversation_max_scroll(&self) -> u16 {
        self.conversation_visual_total
            .saturating_sub(self.conversation_view_height)
    }

    /// Conversation のスクロール位置を上限にクランプする
    fn clamp_conversation_scroll(&mut self) {
        let max = self.conversation_max_scroll();
        if self.conversation_scroll > max {
            self.conversation_scroll = max;
        }
    }

    /// Commit Message のスクロール上限を返す
    fn commit_msg_max_scroll(&self) -> u16 {
        self.commit_msg_visual_total
            .saturating_sub(self.commit_msg_view_height)
    }

    /// Commit Message のスクロール位置を上限にクランプする
    fn clamp_commit_msg_scroll(&mut self) {
        let max = self.commit_msg_max_scroll();
        if self.commit_msg_scroll > max {
            self.commit_msg_scroll = max;
        }
    }

    /// Commit Overview のスクロール上限を返す
    fn commit_overview_max_scroll(&self) -> u16 {
        self.commit_overview_visual_total
            .saturating_sub(self.commit_overview_view_height)
    }

    /// Commit Overview のスクロール位置を上限にクランプする
    fn clamp_commit_overview_scroll(&mut self) {
        let max = self.commit_overview_max_scroll();
        if self.commit_overview_scroll > max {
            self.commit_overview_scroll = max;
        }
    }

    /// 座標からペインを特定
    fn panel_at(&self, x: u16, y: u16) -> Option<Panel> {
        let pos = Position::new(x, y);
        if self.layout.pr_desc_rect.contains(pos) {
            Some(Panel::PrDescription)
        } else if self.layout.commit_list_rect.contains(pos) {
            Some(Panel::CommitList)
        } else if self.layout.file_tree_rect.contains(pos) {
            Some(Panel::FileTree)
        } else if self.layout.conversation_rect.contains(pos) {
            Some(Panel::Conversation)
        } else if self.layout.commit_msg_rect.contains(pos) {
            Some(Panel::CommitMessage)
        } else if self.layout.diff_view_rect.contains(pos) {
            Some(Panel::DiffView)
        } else if self.layout.commit_overview_rect.contains(pos) {
            Some(Panel::CommitList)
        } else {
            None
        }
    }

    /// 行選択モードに入る（hunk header 上では無効）
    fn enter_line_select_mode(&mut self) {
        if self.is_hunk_header(self.diff.cursor_line) {
            return;
        }
        // 現在のカーソル行をアンカーとして選択開始
        self.line_selection = Some(LineSelection {
            anchor: self.diff.cursor_line,
        });
        self.mode = AppMode::LineSelect;
    }

    /// 行選択モードを終了
    fn exit_line_select_mode(&mut self) {
        self.line_selection = None;
        self.mode = AppMode::Normal;
    }

    /// コメント入力モードに入る（行選択がある場合のみ）
    fn enter_comment_input_mode(&mut self) {
        if self.line_selection.is_some() {
            self.review.comment_editor.clear();
            self.mode = AppMode::CommentInput;
        }
    }

    /// コメント入力をキャンセルして Normal モードに戻る（選択範囲もクリア）
    fn cancel_comment_input(&mut self) {
        self.review.comment_editor.clear();
        self.line_selection = None;
        self.mode = AppMode::Normal;
    }

    /// コメントを確定して pending_comments に追加
    fn confirm_comment(&mut self) {
        if self.review.comment_editor.is_empty() {
            return;
        }

        if let Some(selection) = self.line_selection {
            let (start, end) = selection.range(self.diff.cursor_line);
            let file_path = self
                .current_file()
                .map(|f| f.filename.clone())
                .unwrap_or_default();
            let commit_sha = self
                .commit_list_state
                .selected()
                .and_then(|idx| self.commits.get(idx))
                .map(|c| c.sha.clone())
                .unwrap_or_default();

            self.review.pending_comments.push(PendingComment {
                file_path,
                start_line: start,
                end_line: end,
                body: self.review.comment_editor.text(),
                commit_sha,
            });
        }

        self.review.comment_editor.clear();
        self.line_selection = None;
        self.mode = AppMode::Normal;
    }

    /// 選択範囲の diff 行から「新しい側」のコードを抽出する
    fn extract_suggestion_lines(&self, start: usize, end: usize) -> Result<Vec<String>, String> {
        let patch = self
            .current_file()
            .and_then(|f| f.patch.as_deref())
            .ok_or("No patch available")?;
        let lines: Vec<&str> = patch.lines().collect();
        let mut code_lines = Vec::new();
        for i in start..=end {
            if let Some(line) = lines.get(i) {
                if let Some(rest) = line.strip_prefix('+') {
                    code_lines.push(rest.to_string());
                } else if let Some(rest) = line.strip_prefix(' ') {
                    code_lines.push(rest.to_string());
                }
                // '-' 行と '@@' 行は除外
            }
        }
        if code_lines.is_empty() {
            Err("No suggestion-eligible lines in selection".to_string())
        } else {
            Ok(code_lines)
        }
    }

    /// 選択行のコードを suggestion テンプレートとしてエディタに挿入する
    fn insert_suggestion(&mut self) {
        let Some(selection) = self.line_selection else {
            self.status_message = Some(StatusMessage::error("No line selection"));
            return;
        };
        let (start, end) = selection.range(self.diff.cursor_line);
        match self.extract_suggestion_lines(start, end) {
            Ok(code_lines) => {
                let template = format!("```suggestion\n{}\n```", code_lines.join("\n"));
                self.review.comment_editor.insert_text(&template);
            }
            Err(msg) => {
                self.status_message = Some(StatusMessage::error(msg));
            }
        }
    }

    /// owner/repo を分割して (owner, repo) を返す
    fn parse_repo(&self) -> Option<(&str, &str)> {
        let (owner, repo) = self.repo.split_once('/')?;
        if owner.is_empty() || repo.is_empty() {
            return None;
        }
        Some((owner, repo))
    }

    /// レビューを GitHub PR Review API に送信
    fn submit_review_with_event(&mut self, event: ReviewEvent) {
        // COMMENT はコメントが必要
        if event == ReviewEvent::Comment && self.review.pending_comments.is_empty() {
            return;
        }

        let Some(client) = &self.client else {
            self.status_message = Some(StatusMessage::error("✗ No API client available"));
            return;
        };

        let Some((owner, repo)) = self.parse_repo() else {
            self.status_message = Some(StatusMessage::error("✗ Invalid repo format"));
            return;
        };

        // HEAD コミットの SHA を取得
        let Some(head_sha) = self.commits.last().map(|c| c.sha.as_str()) else {
            self.status_message = Some(StatusMessage::error("✗ No commits available"));
            return;
        };

        let count = self.review.pending_comments.len();
        let ctx = review::ReviewContext {
            client,
            owner,
            repo,
            pr_number: self.pr_number,
        };

        // 同期ループ内から async を呼ぶ
        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(review::submit_review(
                &ctx,
                head_sha,
                &self.review.pending_comments,
                &self.files_map,
                event.as_api_str(),
                &self.review.review_body_editor.text(),
            ))
        });

        match result {
            Ok(()) => {
                let msg = if count > 0 {
                    format!(
                        "✓ {} ({} comment{})",
                        event.label(),
                        count,
                        if count == 1 { "" } else { "s" }
                    )
                } else {
                    format!("✓ {}", event.label())
                };
                self.status_message = Some(StatusMessage::info(msg));
                self.review.pending_comments.clear();
                self.review.review_body_editor.clear();
            }
            Err(e) => {
                self.status_message = Some(StatusMessage::error(format!("✗ Failed: {}", e)));
            }
        }
    }

    /// Issue Comment を GitHub API に送信
    fn submit_issue_comment(&mut self) {
        let body = self.review.comment_editor.text();
        if body.trim().is_empty() {
            return;
        }

        let Some(client) = &self.client else {
            self.status_message = Some(StatusMessage::error("✗ No API client available"));
            return;
        };

        let Some((owner, repo)) = self.parse_repo() else {
            self.status_message = Some(StatusMessage::error("✗ Invalid repo format"));
            return;
        };

        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(comments::post_issue_comment(
                client,
                owner,
                repo,
                self.pr_number,
                &body,
            ))
        });

        match result {
            Ok(comment) => {
                self.conversation.push(ConversationEntry {
                    author: comment.user.login,
                    body: comment.body.unwrap_or_default(),
                    created_at: comment.created_at,
                    kind: ConversationKind::IssueComment,
                });
                self.conversation_rendered = None; // キャッシュ無効化
                self.review.comment_editor.clear();
                // 末尾までスクロール（次の render で visual_total が更新されるため大きな値を設定）
                self.conversation_scroll = u16::MAX;
                self.status_message = Some(StatusMessage::info("✓ Comment posted"));
            }
            Err(e) => {
                self.status_message = Some(StatusMessage::error(format!("✗ Failed: {}", e)));
            }
        }
    }

    /// Reply Comment を GitHub API に送信
    fn submit_reply_comment(&mut self) {
        let body = self.review.comment_editor.text();
        if body.trim().is_empty() {
            self.review.reply_to_comment_id = None;
            return;
        }

        let Some(in_reply_to) = self.review.reply_to_comment_id.take() else {
            return;
        };

        let Some(client) = &self.client else {
            self.status_message = Some(StatusMessage::error("✗ No API client available"));
            return;
        };

        let Some((owner, repo)) = self.parse_repo() else {
            self.status_message = Some(StatusMessage::error("✗ Invalid repo format"));
            return;
        };

        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(comments::post_reply_comment(
                client,
                owner,
                repo,
                self.pr_number,
                &body,
                in_reply_to,
            ))
        });

        match result {
            Ok(comment) => {
                // review_comments に追加
                self.review.review_comments.push(comment.clone());

                // viewing_comments が表示中なら追加（CommentView 経由時）
                if !self.review.viewing_comments.is_empty() {
                    self.review.viewing_comments.push(comment.clone());
                }

                // conversation 内の該当 CodeComment エントリに reply を追加
                for entry in &mut self.conversation {
                    if let ConversationKind::CodeComment {
                        root_comment_id,
                        ref mut replies,
                        ..
                    } = entry.kind
                        && root_comment_id == in_reply_to
                    {
                        replies.push(CodeCommentReply {
                            author: comment.user.login.clone(),
                            body: comment.body.clone(),
                            created_at: comment.created_at.clone(),
                        });
                        break;
                    }
                }

                self.conversation_rendered = None; // キャッシュ無効化
                self.review.comment_editor.clear();
                self.status_message = Some(StatusMessage::info("✓ Reply posted"));
            }
            Err(e) => {
                // 失敗時は reply_to_comment_id を復元して再試行可能に
                self.review.reply_to_comment_id = Some(in_reply_to);
                self.status_message = Some(StatusMessage::error(format!("✗ Failed: {}", e)));
            }
        }
    }

    /// CommentView のルートコメント ID から resolve/unresolve をトグルする
    pub(super) fn toggle_resolve_thread(&mut self) {
        let Some(root_id) = comments::root_comment_id(&self.review.viewing_comments) else {
            return;
        };

        let Some(thread) = self.review.thread_map.get(&root_id) else {
            self.status_message = Some(StatusMessage::error("Thread info not available"));
            return;
        };

        let should_resolve = !thread.is_resolved;
        self.review.needs_resolve_toggle = Some(ResolveToggleRequest {
            thread_node_id: thread.node_id.clone(),
            should_resolve,
            root_comment_id: root_id,
        });
    }

    /// resolve/unresolve を実行（draw 後に呼ばれる）
    fn execute_resolve_toggle(&mut self) {
        let Some(req) = self.review.needs_resolve_toggle.take() else {
            return;
        };

        let result = if req.should_resolve {
            comments::resolve_review_thread(&req.thread_node_id)
        } else {
            comments::unresolve_review_thread(&req.thread_node_id)
        };

        match result {
            Ok(is_resolved) if is_resolved == req.should_resolve => {
                // thread_map を更新
                if let Some(thread) = self.review.thread_map.get_mut(&req.root_comment_id) {
                    thread.is_resolved = req.should_resolve;
                }
                // conversation 内の該当エントリを更新
                for entry in &mut self.conversation {
                    if let ConversationKind::CodeComment {
                        ref mut is_resolved,
                        ref thread_node_id,
                        ..
                    } = entry.kind
                        && thread_node_id.as_deref() == Some(&req.thread_node_id)
                    {
                        *is_resolved = req.should_resolve;
                    }
                }
                self.conversation_rendered = None; // キャッシュ無効化
                let label = if req.should_resolve {
                    "✓ Thread resolved"
                } else {
                    "✓ Thread unresolved"
                };
                self.status_message = Some(StatusMessage::info(label));
            }
            Ok(_) => {
                self.status_message = Some(StatusMessage::error(
                    "✗ Operation returned unexpected state",
                ));
            }
            Err(e) => {
                self.status_message = Some(StatusMessage::error(format!("✗ Failed: {}", e)));
            }
        }
    }

    /// PR データをリロードして App 状態を更新する
    fn execute_reload(&mut self) {
        let Some(client) = &self.client else {
            self.status_message = Some(StatusMessage::error("✗ No API client available"));
            return;
        };

        let Some((owner, repo)) = self.parse_repo() else {
            self.status_message = Some(StatusMessage::error("✗ Invalid repo format"));
            return;
        };

        let client = client.clone();
        let owner = owner.to_string();
        let repo = repo.to_string();
        let pr_number = self.pr_number;

        // 状態の保存: 選択中のコミットSHA、ファイル名、パネル状態
        let saved_commit_sha = self.current_commit_sha();
        let saved_filename = self.current_file().map(|f| f.filename.clone());
        let saved_focused_panel = self.focused_panel;
        let saved_zoomed = self.zoomed;
        let saved_viewed_files = self.viewed_files.clone();
        let saved_pending_comments = self.review.pending_comments.clone();

        // block_in_place + block_on で async を呼ぶ（既存パターン踏襲）
        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(crate::reload_pr_data(&client, &owner, &repo, pr_number))
        });

        match result {
            Ok(data) => {
                // PR メタデータを更新
                self.pr_title = data.metadata.pr_title;
                self.pr_body = data.metadata.pr_body;
                self.pr_author = data.metadata.pr_author;
                self.pr_base_branch = data.metadata.pr_base_branch;
                self.pr_head_branch = data.metadata.pr_head_branch;
                self.pr_created_at = data.metadata.pr_created_at;
                self.pr_state = data.metadata.pr_state;

                // コミット・ファイル・コメントを差し替え
                self.commits = data.commits;
                self.files_map = data.files_map;
                self.review.review_comments = data.review_comments.clone();

                // thread_map を再構築
                self.review.thread_map = data
                    .review_threads
                    .into_iter()
                    .map(|t| (t.root_comment_database_id, t))
                    .collect();

                // visible_review_comment_cache を再計算
                self.visible_review_comment_cache = Self::build_visible_comment_cache(
                    &self.review.review_comments,
                    &self.files_map,
                );

                // conversation を再構築
                self.conversation = crate::build_conversation(
                    data.issue_comments,
                    data.reviews,
                    data.review_comments,
                    &self.review.thread_map.values().cloned().collect::<Vec<_>>(),
                );

                // is_own_pr を再判定
                self.is_own_pr =
                    !self.current_user.is_empty() && self.current_user == self.pr_author;

                // キャッシュ無効化
                self.pr_desc_rendered = None;
                self.conversation_rendered = None;
                self.diff.highlight_cache = None;

                // メディア状態リセット（pr_body 更新に追従）
                self.media_refs = Vec::new();
                self.media_protocol_cache.clear();
                self.media_protocol_worker = None;

                // 状態の復元
                self.focused_panel = saved_focused_panel;
                self.zoomed = saved_zoomed;
                self.viewed_files = saved_viewed_files;
                self.review.pending_comments = saved_pending_comments;

                // コミット選択の復元: SHA で再検索
                if let Some(ref sha) = saved_commit_sha {
                    if let Some(idx) = self.commits.iter().position(|c| c.sha == *sha) {
                        self.commit_list_state.select(Some(idx));
                    } else if !self.commits.is_empty() {
                        // 見つからなければ末尾（最新コミット）
                        self.commit_list_state.select(Some(self.commits.len() - 1));
                    } else {
                        self.commit_list_state.select(None);
                    }
                } else if !self.commits.is_empty() {
                    self.commit_list_state.select(Some(0));
                }

                // ファイル選択の復元: ファイル名で再検索
                let files = self.current_files();
                if let Some(ref name) = saved_filename {
                    if let Some(idx) = files.iter().position(|f| f.filename == *name) {
                        self.file_list_state.select(Some(idx));
                    } else if !files.is_empty() {
                        self.file_list_state.select(Some(0));
                    } else {
                        self.file_list_state.select(None);
                    }
                } else if !files.is_empty() {
                    self.file_list_state.select(Some(0));
                } else {
                    self.file_list_state.select(None);
                }

                // Diff 状態をリセット
                self.diff.cursor_line = 0;
                self.diff.scroll = 0;
                let max = self.current_diff_line_count();
                self.diff.cursor_line = self.skip_hunk_header_forward(0, max);
                self.diff.visual_offsets = None;

                // スクロール位置のリセット
                self.pr_desc_scroll = 0;
                self.pr_desc_visual_total = 0;
                self.commit_msg_scroll = 0;
                self.commit_msg_visual_total = 0;
                self.conversation_scroll = 0;
                self.conversation_visual_total = 0;
                self.conversation_cursor = 0;

                self.status_message = Some(StatusMessage::info("✓ Reloaded"));
            }
            Err(e) => {
                self.status_message = Some(StatusMessage::error(format!("✗ Reload failed: {}", e)));
            }
        }
    }

    /// バックグラウンド非同期データの受信・適用
    fn poll_async_data(&mut self) {
        // borrow checker 対策: Option::take() で一時的に取り出す
        let Some(mut rx) = self.async_rx.take() else {
            return;
        };

        let mut disconnected = false;

        // try_recv() ループで全メッセージを処理
        loop {
            match rx.try_recv() {
                Ok(data) => match data {
                    crate::AsyncData::FilesMap(files_map) => {
                        self.apply_files_map(files_map);
                    }
                    crate::AsyncData::ConversationData {
                        review_comments,
                        issue_comments,
                        reviews,
                        review_threads,
                    } => {
                        self.apply_conversation_data(
                            review_comments,
                            issue_comments,
                            reviews,
                            review_threads,
                        );
                    }
                    crate::AsyncData::MediaData(media_cache) => {
                        self.media_cache = media_cache;
                        self.loading.media = LoadPhase::Done;
                    }
                    crate::AsyncData::Error(kind, msg) => {
                        self.status_message =
                            Some(StatusMessage::error(format!("✗ {msg} — press R to retry")));
                        match kind {
                            crate::AsyncErrorKind::Files => {
                                self.loading.files = LoadPhase::Error;
                            }
                            crate::AsyncErrorKind::Conversation => {
                                self.loading.conversation = LoadPhase::Error;
                            }
                            crate::AsyncErrorKind::Media => {
                                self.loading.media = LoadPhase::Error;
                            }
                        }
                    }
                },
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if disconnected || self.loading.all_done() {
            // 全タスク完了 → rx を返却せずに破棄
            // チャネル切断時に Loading のままのフェーズがあればエラーに強制遷移
            if self.loading.files == LoadPhase::Loading {
                self.loading.files = LoadPhase::Error;
            }
            if self.loading.conversation == LoadPhase::Loading {
                self.loading.conversation = LoadPhase::Error;
            }
            if self.loading.media == LoadPhase::Loading {
                self.loading.media = LoadPhase::Error;
            }
            self.try_write_cache();
        } else {
            // まだ受信中 → rx を戻す
            self.async_rx = Some(rx);
        }
    }

    /// files_map をバックグラウンドデータで更新
    fn apply_files_map(&mut self, files_map: HashMap<String, Vec<DiffFile>>) {
        self.files_map = files_map;
        self.loading.files = LoadPhase::Done;

        // visible_review_comment_cache を再計算
        self.visible_review_comment_cache =
            Self::build_visible_comment_cache(&self.review.review_comments, &self.files_map);

        // ファイル選択を初期化
        self.reset_file_selection();

        // diff キャッシュ無効化
        self.diff.highlight_cache = None;
    }

    /// conversation データをバックグラウンドデータで更新
    fn apply_conversation_data(
        &mut self,
        review_comments: Vec<ReviewComment>,
        issue_comments: Vec<crate::github::comments::IssueComment>,
        reviews: Vec<crate::github::review::ReviewSummary>,
        review_threads: Vec<ReviewThread>,
    ) {
        // thread_map を再構築
        self.review.thread_map = review_threads
            .iter()
            .cloned()
            .map(|t| (t.root_comment_database_id, t))
            .collect();

        // visible_review_comment_cache を事前計算（review_comments の参照のみ必要）
        self.visible_review_comment_cache =
            Self::build_visible_comment_cache(&review_comments, &self.files_map);

        // conversation を構築（review_comments の所有権を渡す）
        // build_conversation が所有権を要求するため、self.review.review_comments 用に先に clone
        self.review.review_comments = review_comments.clone();
        self.conversation =
            crate::build_conversation(issue_comments, reviews, review_comments, &review_threads);

        // レンダリングキャッシュ無効化
        self.conversation_rendered = None;

        self.loading.conversation = LoadPhase::Done;
    }

    /// キャッシュ書き込みを試行（files + conversation 両方 Done かつ未書き込みの場合）
    fn try_write_cache(&mut self) {
        if self.cache_written {
            return;
        }
        if self.loading.files != LoadPhase::Done || self.loading.conversation != LoadPhase::Done {
            return;
        }

        let Some((owner, repo)) = self.parse_repo() else {
            return;
        };
        let owner = owner.to_string();
        let repo = repo.to_string();

        let review_threads: Vec<ReviewThread> = self.review.thread_map.values().cloned().collect();

        crate::github::cache::write_cache(
            &owner,
            &repo,
            self.pr_number,
            &crate::github::cache::PrCache {
                version: crate::github::cache::CACHE_VERSION,
                head_sha: self.head_sha.clone(),
                files_map: self.files_map.clone(),
                review_threads,
            },
        );
        self.cache_written = true;
    }

    /// 非同期ロード中かどうかを返す（いずれかのフェーズが Loading）
    pub fn is_async_loading(&self) -> bool {
        self.loading.any_loading()
    }

    /// 選択範囲を下に拡張（カーソルを下に移動）
    fn extend_selection_down(&mut self) {
        let line_count = self.current_diff_line_count();
        let next = self.diff.cursor_line + 1;
        if next < line_count
            && !self.is_hunk_header(next)
            && self.is_same_hunk(self.diff.cursor_line, next)
        {
            self.diff.cursor_line = next;
            self.ensure_cursor_visible();
        }
    }

    /// 選択範囲を上に拡張（カーソルを上に移動）
    fn extend_selection_up(&mut self) {
        if self.diff.cursor_line > 0 {
            let prev = self.diff.cursor_line - 1;
            if !self.is_hunk_header(prev) && self.is_same_hunk(self.diff.cursor_line, prev) {
                self.diff.cursor_line = prev;
                self.ensure_cursor_visible();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::media::process_inline_media;
    use super::*;
    use crate::github::commits::{CommitDetail, CommitInfo};
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::layout::Rect;
    use std::time::{Duration, Instant};
    use unicode_width::UnicodeWidthStr;

    const TEST_SHA_0: &str = "abc1234567890";
    const TEST_SHA_1: &str = "def4567890123";

    fn create_test_commits() -> Vec<CommitInfo> {
        vec![
            CommitInfo {
                sha: TEST_SHA_0.to_string(),
                commit: CommitDetail {
                    message: "First commit".to_string(),
                    author: None,
                },
            },
            CommitInfo {
                sha: TEST_SHA_1.to_string(),
                commit: CommitDetail {
                    message: "Second commit".to_string(),
                    author: None,
                },
            },
        ]
    }

    fn create_test_files() -> Vec<DiffFile> {
        vec![
            DiffFile {
                filename: "src/main.rs".to_string(),
                status: "modified".to_string(),
                additions: 10,
                deletions: 5,
                patch: None,
            },
            DiffFile {
                filename: "src/app.rs".to_string(),
                status: "added".to_string(),
                additions: 50,
                deletions: 0,
                patch: None,
            },
        ]
    }

    fn create_test_files_map(commits: &[CommitInfo]) -> HashMap<String, Vec<DiffFile>> {
        let mut files_map = HashMap::new();
        for commit in commits {
            files_map.insert(commit.sha.clone(), create_test_files());
        }
        files_map
    }

    struct TestAppBuilder {
        pr_number: u64,
        repo: String,
        pr_title: String,
        pr_body: String,
        pr_author: String,
        commits: Vec<CommitInfo>,
        files_map: HashMap<String, Vec<DiffFile>>,
        review_comments: Vec<ReviewComment>,
        client: Option<Octocrab>,
        theme: ThemeMode,
        is_own_pr: bool,
    }

    impl TestAppBuilder {
        fn new() -> Self {
            Self {
                pr_number: 1,
                repo: "owner/repo".to_string(),
                pr_title: "Test PR".to_string(),
                pr_body: String::new(),
                pr_author: String::new(),
                commits: vec![],
                files_map: HashMap::new(),
                review_comments: vec![],
                client: None,
                theme: ThemeMode::Dark,
                is_own_pr: false,
            }
        }

        /// 標準テストコミット + ファイルマップを設定
        fn with_test_data(mut self) -> Self {
            self.commits = create_test_commits();
            self.files_map = create_test_files_map(&self.commits);
            self
        }

        /// 標準テストコミットのみ（ファイルマップなし）
        fn with_commits(mut self) -> Self {
            self.commits = create_test_commits();
            self
        }

        /// カスタムファイルマップを設定
        fn files_map(mut self, files_map: HashMap<String, Vec<DiffFile>>) -> Self {
            self.files_map = files_map;
            self
        }

        /// 10行パッチ付きテストデータを設定（コミットも自動設定される）
        fn with_patch(mut self) -> Self {
            self.commits = create_test_commits();
            let patch = (0..10)
                .map(|i| format!("+line {}", i))
                .collect::<Vec<_>>()
                .join("\n");
            let mut files_map = HashMap::new();
            files_map.insert(
                TEST_SHA_0.to_string(),
                vec![DiffFile {
                    filename: "src/main.rs".to_string(),
                    status: "added".to_string(),
                    additions: 10,
                    deletions: 0,
                    patch: Some(patch),
                }],
            );
            self.files_map = files_map;
            self
        }

        /// カスタムパッチ文字列でテストデータを設定（コミットも自動設定される）
        fn with_custom_patch(
            mut self,
            patch: &str,
            status: &str,
            additions: usize,
            deletions: usize,
        ) -> Self {
            self.commits = create_test_commits();
            let mut files_map = HashMap::new();
            files_map.insert(
                TEST_SHA_0.to_string(),
                vec![DiffFile {
                    filename: "src/main.rs".to_string(),
                    status: status.to_string(),
                    additions,
                    deletions,
                    patch: Some(patch.to_string()),
                }],
            );
            self.files_map = files_map;
            self
        }

        /// レビューコメントを設定
        fn review_comments(mut self, comments: Vec<ReviewComment>) -> Self {
            self.review_comments = comments;
            self
        }

        /// PR本文を設定
        fn pr_body(mut self, body: &str) -> Self {
            self.pr_body = body.to_string();
            self
        }

        /// リポジトリ名を設定
        fn repo(mut self, repo: &str) -> Self {
            self.repo = repo.to_string();
            self
        }

        /// 自分のPRとして設定
        fn own_pr(mut self) -> Self {
            self.is_own_pr = true;
            self
        }

        fn build(self) -> App {
            App::new(
                self.pr_number,
                self.repo,
                self.pr_title,
                self.pr_body,
                self.pr_author,
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                self.commits,
                self.files_map,
                self.review_comments,
                Vec::new(),
                self.client,
                self.theme,
                self.is_own_pr,
                String::new(),
                Vec::new(),
                None, // async_rx
                LoadingState {
                    files: LoadPhase::Done,
                    conversation: LoadPhase::Done,
                    media: LoadPhase::Done,
                }, // loading: テストでは全データロード済み
                String::new(), // head_sha
                true, // cache_written (テスト時は書き込みスキップ)
            )
        }
    }

    #[test]
    fn test_new_with_empty_commits() {
        let app = TestAppBuilder::new().build();
        assert!(!app.should_quit);
        assert_eq!(app.focused_panel, Panel::PrDescription);
        assert_eq!(app.pr_number, 1);
        assert_eq!(app.repo, "owner/repo");
        assert_eq!(app.pr_title, "Test PR");
        assert!(app.commits.is_empty());
        assert_eq!(app.commit_list_state.selected(), None);
        assert!(app.files_map.is_empty());
        assert_eq!(app.file_list_state.selected(), None);
    }

    #[test]
    fn test_new_with_commits() {
        let app = TestAppBuilder::new().with_commits().build();
        assert_eq!(app.commits.len(), 2);
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    #[test]
    fn test_new_with_files() {
        let app = TestAppBuilder::new().with_test_data().build();
        assert_eq!(app.files_map.len(), 2);
        assert_eq!(app.file_list_state.selected(), Some(0));
    }

    #[test]
    fn test_next_panel() {
        let mut app = TestAppBuilder::new().build();
        assert_eq!(app.focused_panel, Panel::PrDescription);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::PrDescription);
    }

    #[test]
    fn test_prev_panel() {
        let mut app = TestAppBuilder::new().build();
        assert_eq!(app.focused_panel, Panel::PrDescription);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::PrDescription);
    }

    #[test]
    fn test_select_next_commits() {
        let mut app = TestAppBuilder::new().with_commits().build();
        app.focused_panel = Panel::CommitList;
        assert_eq!(app.commit_list_state.selected(), Some(0));
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1));
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1)); // clamped at end
    }

    #[test]
    fn test_select_prev_commits() {
        let mut app = TestAppBuilder::new().with_commits().build();
        app.focused_panel = Panel::CommitList;
        assert_eq!(app.commit_list_state.selected(), Some(0));
        app.select_prev();
        assert_eq!(app.commit_list_state.selected(), Some(0)); // clamped at start
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1));
        app.select_prev();
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    #[test]
    fn test_select_next_files() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.focused_panel = Panel::FileTree;
        assert_eq!(app.file_list_state.selected(), Some(0));
        app.select_next();
        assert_eq!(app.file_list_state.selected(), Some(1));
        app.select_next();
        assert_eq!(app.file_list_state.selected(), Some(1)); // clamped at end
    }

    #[test]
    fn test_select_prev_files() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.focused_panel = Panel::FileTree;
        assert_eq!(app.file_list_state.selected(), Some(0));
        app.select_prev();
        assert_eq!(app.file_list_state.selected(), Some(0)); // clamped at start
        app.select_next();
        assert_eq!(app.file_list_state.selected(), Some(1));
        app.select_prev();
        assert_eq!(app.file_list_state.selected(), Some(0));
    }

    #[test]
    fn test_select_only_works_in_current_panel() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.focused_panel = Panel::CommitList;
        // Initial state: CommitList panel
        // コミット選択変更時にファイル選択がリセットされることを確認
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1));
        assert_eq!(app.file_list_state.selected(), Some(0)); // reset to first file

        // Move to FileTree panel
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1)); // commits unchanged
        assert_eq!(app.file_list_state.selected(), Some(1));
    }

    #[test]
    fn test_commit_list_state() {
        let app = TestAppBuilder::new().with_commits().build();

        // Verify the commit list state is properly initialized
        assert_eq!(app.commit_list_state.selected(), Some(0));
        assert_eq!(app.commits.len(), 2);
        assert_eq!(app.commits[0].short_sha(), "abc1234");
        assert_eq!(app.commits[0].message_summary(), "First commit");
    }

    #[test]
    fn test_current_files_returns_correct_files() {
        let mut files_map = HashMap::new();
        files_map.insert(
            TEST_SHA_0.to_string(),
            vec![DiffFile {
                filename: "file1.rs".to_string(),
                status: "added".to_string(),
                additions: 10,
                deletions: 0,
                patch: None,
            }],
        );
        files_map.insert(
            TEST_SHA_1.to_string(),
            vec![DiffFile {
                filename: "file2.rs".to_string(),
                status: "modified".to_string(),
                additions: 5,
                deletions: 3,
                patch: None,
            }],
        );

        let app = TestAppBuilder::new()
            .with_commits()
            .files_map(files_map)
            .build();

        // 最初のコミットのファイルが返される
        let files = app.current_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "file1.rs");
    }

    #[test]
    fn test_commit_change_resets_file_selection() {
        let mut files_map = HashMap::new();
        files_map.insert(
            TEST_SHA_0.to_string(),
            vec![
                DiffFile {
                    filename: "file1.rs".to_string(),
                    status: "added".to_string(),
                    additions: 10,
                    deletions: 0,
                    patch: None,
                },
                DiffFile {
                    filename: "file2.rs".to_string(),
                    status: "added".to_string(),
                    additions: 5,
                    deletions: 0,
                    patch: None,
                },
            ],
        );
        files_map.insert(
            TEST_SHA_1.to_string(),
            vec![DiffFile {
                filename: "file3.rs".to_string(),
                status: "modified".to_string(),
                additions: 5,
                deletions: 3,
                patch: None,
            }],
        );

        let mut app = TestAppBuilder::new()
            .with_commits()
            .files_map(files_map)
            .build();

        // ファイル一覧に移動して2番目のファイルを選択
        app.focused_panel = Panel::FileTree;
        app.select_next();
        assert_eq!(app.file_list_state.selected(), Some(1));

        // コミット一覧に戻ってコミットを変更
        app.prev_panel();
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1));

        // ファイル選択がリセットされていることを確認
        assert_eq!(app.file_list_state.selected(), Some(0));

        // 新しいコミットのファイルが取得できることを確認
        let files = app.current_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "file3.rs");
    }

    #[test]
    fn test_diff_scroll_initial() {
        let app = TestAppBuilder::new().with_commits().build();
        assert_eq!(app.diff.scroll, 0);
    }

    #[test]
    fn test_scroll_diff_down() {
        // 10行パッチ、half page = 5
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.view_height = 10;
        assert_eq!(app.diff.cursor_line, 0);

        app.scroll_diff_down();
        assert_eq!(app.diff.cursor_line, 5); // 半ページ分

        app.scroll_diff_down();
        assert_eq!(app.diff.cursor_line, 9); // 末尾でクランプ (10行-1)
    }

    #[test]
    fn test_scroll_diff_up() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.view_height = 10;
        app.diff.cursor_line = 9;

        app.scroll_diff_up();
        assert_eq!(app.diff.cursor_line, 4); // 半ページ分戻る

        app.scroll_diff_up();
        assert_eq!(app.diff.cursor_line, 0);

        // 0 以下にはならない
        app.scroll_diff_up();
        assert_eq!(app.diff.cursor_line, 0);
    }

    #[test]
    fn test_scroll_only_works_in_diff_panel() {
        let mut app = create_app_with_patch();
        app.diff.view_height = 10;

        // PrDescription panel (default)
        app.scroll_diff_down();
        assert_eq!(app.diff.cursor_line, 0);

        app.focused_panel = Panel::CommitList;
        app.scroll_diff_down();
        assert_eq!(app.diff.cursor_line, 0);

        app.focused_panel = Panel::FileTree;
        app.scroll_diff_down();
        assert_eq!(app.diff.cursor_line, 0);

        app.focused_panel = Panel::DiffView;
        app.scroll_diff_down();
        assert_eq!(app.diff.cursor_line, 5); // 半ページ分
    }

    #[test]
    fn test_scroll_diff_to_end() {
        let mut files_map = HashMap::new();
        // 25行のパッチ
        let patch = (0..25)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        files_map.insert(
            TEST_SHA_0.to_string(),
            vec![DiffFile {
                filename: "file1.rs".to_string(),
                status: "added".to_string(),
                additions: 25,
                deletions: 0,
                patch: Some(patch),
            }],
        );
        let mut app = TestAppBuilder::new()
            .with_commits()
            .files_map(files_map)
            .build();
        app.focused_panel = Panel::DiffView;

        app.scroll_diff_to_end();
        assert_eq!(app.diff.cursor_line, 24); // 末尾行 (25-1)
    }

    #[test]
    fn test_file_change_resets_scroll() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.diff.scroll = 50;

        // Change to FileTree and select next file
        app.focused_panel = Panel::FileTree;
        app.select_next();

        // Scroll should be reset
        assert_eq!(app.diff.scroll, 0);
    }

    /// コメント入力テスト用: patch 付きファイルを含む App を作成
    fn create_app_with_patch() -> App {
        TestAppBuilder::new().with_patch().build()
    }

    #[test]
    fn test_comment_input_mode_transition_from_line_select() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;

        // 行選択モードに入る
        app.enter_line_select_mode();
        assert_eq!(app.mode, AppMode::LineSelect);
        assert!(app.line_selection.is_some());

        // 'c' でコメント入力モードに遷移
        app.enter_comment_input_mode();
        assert_eq!(app.mode, AppMode::CommentInput);
        assert!(app.review.comment_editor.is_empty());
    }

    #[test]
    fn test_comment_input_mode_cancel_returns_to_normal() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;

        // 行選択 → コメント入力
        app.enter_line_select_mode();
        app.enter_comment_input_mode();
        assert_eq!(app.mode, AppMode::CommentInput);

        // Esc で Normal に戻る（選択範囲もクリア）
        app.cancel_comment_input();
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.line_selection, None);
    }

    #[test]
    fn test_comment_input_char_and_backspace() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.enter_line_select_mode();
        app.enter_comment_input_mode();

        // 文字入力
        app.handle_comment_input_mode(KeyCode::Char('H'), KeyModifiers::NONE);
        app.handle_comment_input_mode(KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(app.review.comment_editor.text(), "Hi");

        // Backspace
        app.handle_comment_input_mode(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.review.comment_editor.text(), "H");

        // 全文字削除
        app.handle_comment_input_mode(KeyCode::Backspace, KeyModifiers::NONE);
        assert!(app.review.comment_editor.is_empty());

        // 空の状態でさらに Backspace しても panic しない
        app.handle_comment_input_mode(KeyCode::Backspace, KeyModifiers::NONE);
        assert!(app.review.comment_editor.is_empty());
    }

    #[test]
    fn test_comment_confirm_adds_pending_comment() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.enter_line_select_mode();
        app.enter_comment_input_mode();

        // コメント入力
        app.handle_comment_input_mode(KeyCode::Char('L'), KeyModifiers::NONE);
        app.handle_comment_input_mode(KeyCode::Char('G'), KeyModifiers::NONE);
        app.handle_comment_input_mode(KeyCode::Char('T'), KeyModifiers::NONE);
        app.handle_comment_input_mode(KeyCode::Char('M'), KeyModifiers::NONE);

        // Enter で確定
        app.confirm_comment();
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.review.pending_comments.len(), 1);
        assert_eq!(app.review.pending_comments[0].body, "LGTM");
        assert_eq!(app.review.pending_comments[0].file_path, "src/main.rs");
        assert!(app.line_selection.is_none());
    }

    #[test]
    fn test_empty_comment_not_saved() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.enter_line_select_mode();
        app.enter_comment_input_mode();

        // 空のまま Enter
        app.confirm_comment();
        assert_eq!(app.mode, AppMode::CommentInput);
        assert!(app.review.pending_comments.is_empty());
    }

    #[test]
    fn test_comment_input_mode_requires_line_selection() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;

        // line_selection が None の状態で遷移しようとしても遷移しない
        assert!(app.line_selection.is_none());
        app.enter_comment_input_mode();
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn test_insert_suggestion_basic() {
        // +行のみのパッチで suggestion テンプレートが挿入される
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.enter_line_select_mode();
        app.enter_comment_input_mode();

        app.insert_suggestion();
        let text = app.review.comment_editor.text();
        assert!(text.starts_with("```suggestion\n"));
        assert!(text.ends_with("\n```"));
        assert!(text.contains("line 0"));
    }

    #[test]
    fn test_insert_suggestion_mixed_lines() {
        // +行、-行、コンテキスト行が混在するパッチ
        let patch = "@@ -1,3 +1,3 @@\n old line\n-removed\n+added";
        let mut app = TestAppBuilder::new()
            .with_custom_patch(patch, "modified", 1, 1)
            .build();
        app.focused_panel = Panel::DiffView;
        // hunk header をスキップ: カーソルを1行目に
        app.diff.cursor_line = 1;
        app.line_selection = Some(LineSelection { anchor: 1 });
        // 3行選択（行1〜3）
        app.diff.cursor_line = 3;
        app.mode = AppMode::CommentInput;

        app.insert_suggestion();
        let text = app.review.comment_editor.text();
        // コンテキスト行 " old line" → "old line" と +行 "+added" → "added" が含まれる
        assert!(text.contains("old line"));
        assert!(text.contains("added"));
        // -行 "-removed" は除外される
        assert!(!text.contains("removed"));
    }

    #[test]
    fn test_insert_suggestion_all_deletions_error() {
        // 全行が -行のパッチ → エラー
        let patch = "@@ -1,2 +0,0 @@\n-deleted1\n-deleted2";
        let mut app = TestAppBuilder::new()
            .with_custom_patch(patch, "modified", 0, 2)
            .build();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 1;
        app.line_selection = Some(LineSelection { anchor: 1 });
        app.diff.cursor_line = 2;
        app.mode = AppMode::CommentInput;

        app.insert_suggestion();
        // エディタは空のまま
        assert!(app.review.comment_editor.is_empty());
        // エラーメッセージが設定される
        assert!(app.status_message.is_some());
        assert_eq!(app.status_message.unwrap().level, StatusLevel::Error);
    }

    #[test]
    fn test_ctrl_g_in_comment_input() {
        // Ctrl+G で insert_suggestion が呼ばれることを handler 経由で確認
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.enter_line_select_mode();
        app.enter_comment_input_mode();

        app.handle_comment_input_mode(KeyCode::Char('g'), KeyModifiers::CONTROL);
        let text = app.review.comment_editor.text();
        assert!(text.starts_with("```suggestion\n"));
        assert!(text.ends_with("\n```"));
    }

    #[test]
    fn test_parse_repo_valid() {
        let app = TestAppBuilder::new().build();
        let (owner, repo) = app.parse_repo().unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_repo_invalid() {
        let app = TestAppBuilder::new().repo("invalid").build();
        assert!(app.parse_repo().is_none());
    }

    #[test]
    fn test_submit_with_empty_pending_comments_does_nothing() {
        let mut app = TestAppBuilder::new().build();
        // pending_comments が空なら何もしない（status_message も None のまま）
        app.submit_review_with_event(ReviewEvent::Comment);
        assert!(app.status_message.is_none());
    }

    #[test]
    fn test_status_message_info() {
        let msg = StatusMessage::info("hello");
        assert_eq!(msg.body, "hello");
        assert_eq!(msg.level, StatusLevel::Info);
        assert!(!msg.is_expired());
    }

    #[test]
    fn test_status_message_error() {
        let msg = StatusMessage::error("oops");
        assert_eq!(msg.body, "oops");
        assert_eq!(msg.level, StatusLevel::Error);
        assert!(!msg.is_expired());
    }

    #[test]
    fn test_status_message_is_expired() {
        let msg = StatusMessage {
            body: "old".to_string(),
            level: StatusLevel::Info,
            created_at: Instant::now() - Duration::from_secs(4),
        };
        assert!(msg.is_expired());

        let msg_fresh = StatusMessage::info("new");
        assert!(!msg_fresh.is_expired());
    }

    #[test]
    fn test_s_key_opens_review_submit_dialog() {
        let mut app = create_app_with_patch();

        // S キーで ReviewSubmit モードに遷移
        app.handle_normal_mode(KeyCode::Char('S'), KeyModifiers::SHIFT);
        assert_eq!(app.mode, AppMode::ReviewSubmit);
        assert_eq!(app.review.review_event_cursor, 0);
    }

    #[test]
    fn test_review_submit_dialog_navigation() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;
        app.review.review_event_cursor = 0;

        // j で下に移動
        app.handle_review_submit_mode(KeyCode::Char('j'));
        assert_eq!(app.review.review_event_cursor, 1);
        app.handle_review_submit_mode(KeyCode::Char('j'));
        assert_eq!(app.review.review_event_cursor, 2);
        // 循環
        app.handle_review_submit_mode(KeyCode::Char('j'));
        assert_eq!(app.review.review_event_cursor, 0);

        // k で上に移動（循環）
        app.handle_review_submit_mode(KeyCode::Char('k'));
        assert_eq!(app.review.review_event_cursor, 2);
    }

    #[test]
    fn test_review_submit_comment_requires_pending() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;
        app.review.review_event_cursor = 0; // Comment

        // pending_comments が空で Comment を選択するとエラー
        app.handle_review_submit_mode(KeyCode::Enter);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.review.needs_submit.is_none());
        assert!(app.status_message.is_some());
        assert_eq!(
            app.status_message.as_ref().unwrap().level,
            StatusLevel::Error
        );
    }

    #[test]
    fn test_review_submit_approve_transitions_to_body_input() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;
        app.review.review_event_cursor = 1; // Approve

        // pending_comments が空でも Approve → ReviewBodyInput に遷移
        app.handle_review_submit_mode(KeyCode::Enter);
        assert_eq!(app.mode, AppMode::ReviewBodyInput);
        assert!(app.review.review_body_editor.is_empty());
        assert!(app.review.needs_submit.is_none());
    }

    #[test]
    fn test_review_submit_escape_cancels() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;

        app.handle_review_submit_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.review.needs_submit.is_none());
        assert!(!app.review.quit_after_submit);
    }

    #[test]
    fn test_review_submit_escape_resets_quit_after_submit() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;
        app.review.quit_after_submit = true; // QuitConfirm → y → ReviewSubmit の流れ

        app.handle_review_submit_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(!app.review.quit_after_submit);
    }

    #[test]
    fn test_number_keys_jump_to_panels() {
        let mut app = TestAppBuilder::new().build();
        app.handle_normal_mode(KeyCode::Char('2'), KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::CommitList);
        app.handle_normal_mode(KeyCode::Char('3'), KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.handle_normal_mode(KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::PrDescription);
    }

    #[test]
    fn test_enter_in_files_moves_to_diff() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.focused_panel = Panel::FileTree;
        app.handle_normal_mode(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::DiffView);
    }

    #[test]
    fn test_esc_in_diff_returns_to_files() {
        let mut app = TestAppBuilder::new().build();
        app.focused_panel = Panel::DiffView;
        app.handle_normal_mode(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::FileTree);
    }

    #[test]
    fn test_tab_skips_diffview() {
        let mut app = TestAppBuilder::new().build();
        // PrDescription → CommitList → FileTree → PrDescription (DiffView をスキップ)
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::PrDescription);
    }

    #[test]
    fn test_diffview_tab_is_noop() {
        let mut app = TestAppBuilder::new().build();
        app.focused_panel = Panel::DiffView;
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::DiffView); // Tab は無効
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::DiffView); // BackTab も無効
    }

    #[test]
    fn test_submit_without_client_sets_error() {
        let mut app = create_app_with_patch();

        // コメントを追加（client は None）
        app.review.pending_comments.push(PendingComment {
            file_path: "test.rs".to_string(),
            start_line: 0,
            end_line: 0,
            body: "test".to_string(),
            commit_sha: "abc".to_string(),
        });

        app.submit_review_with_event(ReviewEvent::Comment);
        assert!(app.status_message.is_some());
        assert_eq!(
            app.status_message.as_ref().unwrap().level,
            StatusLevel::Error
        );
    }

    // === N2: Diff 表示の改善テスト ===

    #[test]
    fn test_status_char_color_mapping() {
        // 各ステータスが正しい文字を返すことを確認
        let added = DiffFile {
            filename: "new.rs".to_string(),
            status: "added".to_string(),
            additions: 10,
            deletions: 0,
            patch: None,
        };
        assert_eq!(added.status_char(), 'A');

        let modified = DiffFile {
            filename: "mod.rs".to_string(),
            status: "modified".to_string(),
            additions: 5,
            deletions: 3,
            patch: None,
        };
        assert_eq!(modified.status_char(), 'M');

        let removed = DiffFile {
            filename: "old.rs".to_string(),
            status: "removed".to_string(),
            additions: 0,
            deletions: 10,
            patch: None,
        };
        assert_eq!(removed.status_char(), 'D');

        let renamed = DiffFile {
            filename: "renamed.rs".to_string(),
            status: "renamed".to_string(),
            additions: 0,
            deletions: 0,
            patch: None,
        };
        assert_eq!(renamed.status_char(), 'R');
    }

    #[test]
    fn test_binary_file_has_no_patch() {
        // patch が None のファイルに対して current_diff_line_count が 0 を返す
        let mut files_map = HashMap::new();
        files_map.insert(
            TEST_SHA_0.to_string(),
            vec![DiffFile {
                filename: "image.png".to_string(),
                status: "added".to_string(),
                additions: 0,
                deletions: 0,
                patch: None,
            }],
        );
        let app = TestAppBuilder::new()
            .with_commits()
            .files_map(files_map)
            .build();
        assert_eq!(app.current_diff_line_count(), 0);
    }

    #[test]
    fn test_commit_message_summary_vs_full() {
        // message_summary は1行目のみ、commit.message は全文
        let commit = CommitInfo {
            sha: TEST_SHA_0.to_string(),
            commit: CommitDetail {
                message: "First line\n\nDetailed description\nMore details".to_string(),
                author: None,
            },
        };
        assert_eq!(commit.message_summary(), "First line");
        assert_eq!(commit.commit.message.lines().count(), 4);
    }

    // === N3: コメント機能の強化テスト ===

    #[test]
    fn test_c_key_single_line_comment_in_diffview() {
        // DiffView で c キーを押すと単一行コメントモードに入る
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 3;

        // Normal モードで c キー
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::empty());
        assert_eq!(app.mode, AppMode::CommentInput);
        assert!(app.line_selection.is_some());

        // line_selection のアンカーがカーソル行に設定されている
        let sel = app.line_selection.unwrap();
        assert_eq!(sel.anchor, 3);
        // 単一行なので range は (3, 3)
        assert_eq!(sel.range(app.diff.cursor_line), (3, 3));
    }

    #[test]
    fn test_c_key_does_nothing_outside_diffview() {
        // DiffView 以外のパネルでは c キーは無効
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::FileTree;

        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::empty());
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.line_selection.is_none());
    }

    #[test]
    fn test_pending_comment_marks_file() {
        // ペンディングコメントがあるファイルを識別できる
        let mut app = create_app_with_patch();
        app.review.pending_comments.push(PendingComment {
            file_path: "src/main.rs".to_string(),
            start_line: 2,
            end_line: 4,
            body: "Review this".to_string(),
            commit_sha: TEST_SHA_0.to_string(),
        });

        // 該当ファイルにペンディングコメントがある
        assert!(
            app.review
                .pending_comments
                .iter()
                .any(|c| c.file_path == "src/main.rs")
        );
        // 別のファイルにはない
        assert!(
            !app.review
                .pending_comments
                .iter()
                .any(|c| c.file_path == "other.rs")
        );
    }

    // === N4: レビューフローの改善テスト ===

    #[test]
    fn test_quit_with_pending_comments_shows_confirm() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;

        // コメントを追加
        app.review.pending_comments.push(PendingComment {
            file_path: "src/main.rs".to_string(),
            start_line: 0,
            end_line: 0,
            body: "test".to_string(),
            commit_sha: TEST_SHA_0.to_string(),
        });

        // q キーで QuitConfirm モードに遷移
        app.handle_normal_mode(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::QuitConfirm);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_quit_without_pending_comments_quits_immediately() {
        let mut app = create_app_with_patch();

        // pending_comments が空なら即終了
        app.handle_normal_mode(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.should_quit);
    }

    #[test]
    fn test_quit_confirm_y_opens_review_submit() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::QuitConfirm;
        app.review.pending_comments.push(PendingComment {
            file_path: "test.rs".to_string(),
            start_line: 0,
            end_line: 0,
            body: "test".to_string(),
            commit_sha: "abc".to_string(),
        });

        // y → ReviewSubmit ダイアログに遷移（quit_after_submit フラグ付き）
        app.handle_quit_confirm_mode(KeyCode::Char('y'));
        assert_eq!(app.mode, AppMode::ReviewSubmit);
        assert!(app.review.quit_after_submit);
        assert_eq!(app.review.review_event_cursor, 0);
    }

    #[test]
    fn test_quit_confirm_n_discards_and_quits() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::QuitConfirm;
        app.review.pending_comments.push(PendingComment {
            file_path: "test.rs".to_string(),
            start_line: 0,
            end_line: 0,
            body: "test".to_string(),
            commit_sha: "abc".to_string(),
        });

        app.handle_quit_confirm_mode(KeyCode::Char('n'));
        assert!(app.should_quit);
        assert!(app.review.pending_comments.is_empty());
    }

    #[test]
    fn test_quit_confirm_c_cancels() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::QuitConfirm;

        app.handle_quit_confirm_mode(KeyCode::Char('c'));
        assert_eq!(app.mode, AppMode::Normal);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_quit_confirm_esc_cancels() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::QuitConfirm;

        app.handle_quit_confirm_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_review_event_api_str() {
        assert_eq!(ReviewEvent::Comment.as_api_str(), "COMMENT");
        assert_eq!(ReviewEvent::Approve.as_api_str(), "APPROVE");
        assert_eq!(ReviewEvent::RequestChanges.as_api_str(), "REQUEST_CHANGES");
    }

    #[test]
    fn test_review_event_label() {
        assert_eq!(ReviewEvent::Comment.label(), "Comment");
        assert_eq!(ReviewEvent::Approve.label(), "Approve");
        assert_eq!(ReviewEvent::RequestChanges.label(), "Request Changes");
    }

    // === N5: 入力方法の拡張テスト ===

    #[test]
    fn test_arrow_keys_select_next_prev() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.focused_panel = Panel::CommitList;

        // Down キーで j と同じ動作
        app.handle_normal_mode(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.commit_list_state.selected(), Some(1));

        // Up キーで k と同じ動作
        app.handle_normal_mode(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    #[test]
    fn test_h_l_panel_navigation() {
        let mut app = TestAppBuilder::new().build();
        assert_eq!(app.focused_panel, Panel::PrDescription);

        // l → 次のパネル
        app.handle_normal_mode(KeyCode::Char('l'), KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::CommitList);

        // Right → 次のパネル
        app.handle_normal_mode(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::FileTree);

        // h → 前のパネル
        app.handle_normal_mode(KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::CommitList);

        // Left → 前のパネル
        app.handle_normal_mode(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::PrDescription);
    }

    #[test]
    fn test_arrow_keys_in_line_select_mode() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.enter_line_select_mode();

        // Down で選択拡張
        app.handle_line_select_mode(KeyCode::Down);
        assert_eq!(app.diff.cursor_line, 1);

        // Up で選択縮小
        app.handle_line_select_mode(KeyCode::Up);
        assert_eq!(app.diff.cursor_line, 0);
    }

    #[test]
    fn test_panel_at_returns_correct_panel() {
        let mut app = create_app_with_patch();
        // Rect を手動設定（render を経由しないテスト用）
        app.layout.pr_desc_rect = Rect::new(0, 1, 30, 10);
        app.layout.commit_list_rect = Rect::new(0, 11, 30, 10);
        app.layout.file_tree_rect = Rect::new(0, 21, 30, 10);
        app.layout.diff_view_rect = Rect::new(30, 1, 50, 30);

        assert_eq!(app.panel_at(5, 5), Some(Panel::PrDescription));
        assert_eq!(app.panel_at(5, 15), Some(Panel::CommitList));
        assert_eq!(app.panel_at(5, 25), Some(Panel::FileTree));
        assert_eq!(app.panel_at(40, 10), Some(Panel::DiffView));
        assert_eq!(app.panel_at(90, 90), None);
    }

    #[test]
    fn test_mouse_click_changes_focus() {
        let mut app = create_app_with_patch();
        app.layout.pr_desc_rect = Rect::new(0, 1, 30, 10);
        app.layout.commit_list_rect = Rect::new(0, 11, 30, 10);
        app.layout.file_tree_rect = Rect::new(0, 21, 30, 10);
        app.layout.diff_view_rect = Rect::new(30, 1, 50, 30);

        assert_eq!(app.focused_panel, Panel::PrDescription);

        app.handle_mouse_click(40, 10);
        assert_eq!(app.focused_panel, Panel::DiffView);

        app.handle_mouse_click(5, 15);
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_mouse_click_selects_list_item() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        // CommitList: y=11 はボーダー、y=12 が最初のアイテム
        app.layout.commit_list_rect = Rect::new(0, 11, 30, 10);

        // 2番目のアイテム（y=13, offset 0, relative_y=1 → idx=1）をクリック
        app.handle_mouse_click(5, 13);
        assert_eq!(app.focused_panel, Panel::CommitList);
        assert_eq!(app.commit_list_state.selected(), Some(1));
    }

    #[test]
    fn test_mouse_scroll_on_diff() {
        // 10行パッチ、表示5行 → max_scroll = 5
        let mut app = create_app_with_patch();
        app.layout.diff_view_rect = Rect::new(30, 1, 50, 30);
        app.diff.view_height = 5;
        app.focused_panel = Panel::FileTree; // フォーカスは別のペイン

        // 下スクロール → ビューポート+カーソル同時移動（見た目位置固定）
        assert_eq!(app.diff.cursor_line, 0);
        assert_eq!(app.diff.scroll, 0);
        app.handle_mouse_scroll(40, 10, true);
        assert_eq!(app.diff.cursor_line, 1);
        assert_eq!(app.diff.scroll, 1);

        // 上スクロール → 元に戻る
        app.handle_mouse_scroll(40, 10, false);
        assert_eq!(app.diff.cursor_line, 0);
        assert_eq!(app.diff.scroll, 0);

        // ページ先頭で上スクロール → カーソルのみ（既に0なので動かない）
        app.handle_mouse_scroll(40, 10, false);
        assert_eq!(app.diff.cursor_line, 0);
        assert_eq!(app.diff.scroll, 0);

        // ページ末尾まで下スクロール（max_scroll=5）
        for _ in 0..5 {
            app.handle_mouse_scroll(40, 10, true);
        }
        assert_eq!(app.diff.scroll, 5);
        assert_eq!(app.diff.cursor_line, 5);

        // ページ末尾到達後 → カーソルのみ移動
        app.handle_mouse_scroll(40, 10, true);
        assert_eq!(app.diff.scroll, 5); // ページは動かない
        assert_eq!(app.diff.cursor_line, 6); // カーソルだけ進む

        assert_eq!(app.focused_panel, Panel::FileTree); // フォーカスは変わらない
    }

    #[test]
    fn test_mouse_scroll_on_pr_description() {
        // マークダウンではパラグラフ間に空行が必要（連続行は1段落として結合される）
        let mut app = TestAppBuilder::new()
            .pr_body("line1\n\nline2\n\nline3\n\nline4\n\nline5")
            .build();
        app.layout.pr_desc_rect = Rect::new(0, 1, 30, 5);
        app.pr_desc_view_height = 3;
        // ensure_pr_desc_rendered でキャッシュを生成
        app.ensure_pr_desc_rendered();

        // total_lines > view_height ならスクロール可能
        assert!(app.pr_desc_total_lines() > app.pr_desc_view_height);
        assert_eq!(app.pr_desc_scroll, 0);
        app.handle_mouse_scroll(5, 3, true);
        assert_eq!(app.pr_desc_scroll, 1);
        app.handle_mouse_scroll(5, 3, false);
        assert_eq!(app.pr_desc_scroll, 0);

        // pr_desc_visual_total が設定されている場合はそちらを優先
        app.pr_desc_visual_total = 20;
        assert_eq!(app.pr_desc_total_lines(), 20);
    }

    #[test]
    fn test_mouse_scroll_on_commit_list_ignored() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.layout.commit_list_rect = Rect::new(0, 11, 30, 10);

        // CommitList 上でスクロールしても選択は変わらない
        app.handle_mouse_scroll(5, 15, true);
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    // === N6: viewed フラグテスト ===

    #[test]
    fn test_toggle_viewed() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.focused_panel = Panel::FileTree;
        assert!(app.viewed_files.is_empty());

        // トグル → viewed に追加
        app.toggle_viewed();
        assert!(app.is_file_viewed(TEST_SHA_0, "src/main.rs"));

        // 再トグル → viewed から削除
        app.toggle_viewed();
        assert!(!app.is_file_viewed(TEST_SHA_0, "src/main.rs"));
    }

    #[test]
    fn test_viewed_is_per_commit() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.focused_panel = Panel::FileTree;

        // コミット0 のファイルを viewed にする
        app.toggle_viewed();
        assert!(app.is_file_viewed(TEST_SHA_0, "src/main.rs"));

        // コミットを切り替え
        app.focused_panel = Panel::CommitList;
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1));

        // コミット1 の同名ファイルは viewed でない
        assert!(!app.is_file_viewed(TEST_SHA_1, "src/main.rs"));
    }

    #[test]
    fn test_toggle_viewed_no_file_selected() {
        let mut app = TestAppBuilder::new().build();

        // ファイル未選択時は何もしない（パニックしない）
        app.toggle_viewed();
        assert!(app.viewed_files.is_empty());
    }

    #[test]
    fn test_x_key_toggles_viewed_in_file_tree() {
        let mut app = TestAppBuilder::new().with_test_data().build();
        app.focused_panel = Panel::FileTree;

        // x キーで viewed トグル
        app.handle_normal_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.is_file_viewed(TEST_SHA_0, "src/main.rs"));

        // CommitList では x キーでコミットの全ファイルをトグル
        app.focused_panel = Panel::CommitList;
        app.handle_normal_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        // コミット0 の全ファイル (src/main.rs, src/app.rs) が viewed に
        assert!(app.is_file_viewed(TEST_SHA_0, "src/main.rs"));
        assert!(app.is_file_viewed(TEST_SHA_0, "src/app.rs"));

        // もう一度 x → 全ファイルが unview（既に全て viewed なので）
        app.handle_normal_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(!app.is_file_viewed(TEST_SHA_0, "src/main.rs"));
        assert!(!app.is_file_viewed(TEST_SHA_0, "src/app.rs"));
    }

    // === N6: コメント表示テスト ===

    fn make_review_comment(
        path: &str,
        line: Option<usize>,
        side: &str,
        body: &str,
    ) -> ReviewComment {
        ReviewComment {
            id: 1,
            body: body.to_string(),
            path: path.to_string(),
            line,
            start_line: None,
            side: Some(side.to_string()),
            start_side: None,
            commit_id: TEST_SHA_0.to_string(),
            user: crate::github::comments::ReviewCommentUser {
                login: "testuser".to_string(),
            },
            created_at: "2025-01-01T00:00:00Z".to_string(),
            in_reply_to_id: None,
        }
    }

    fn create_app_with_comments() -> App {
        let comments = vec![make_review_comment(
            "src/main.rs",
            Some(2),
            "RIGHT",
            "Nice line!",
        )];
        TestAppBuilder::new()
            .with_custom_patch("@@ -0,0 +1,3 @@\n+line1\n+line2\n+line3", "added", 3, 0)
            .review_comments(comments)
            .build()
    }

    #[test]
    fn test_existing_comment_counts_maps_correctly() {
        let app = create_app_with_comments();
        let counts = app.existing_comment_counts();
        // line=2 (RIGHT) → patch行: @@ は idx 0, +line1 は idx 1, +line2 は idx 2
        assert_eq!(counts.get(&2), Some(&1));
        // 他の行にはコメントがない
        assert_eq!(counts.get(&0), None);
        assert_eq!(counts.get(&1), None);
        assert_eq!(counts.get(&3), None);
    }

    #[test]
    fn test_existing_comment_counts_outdated_skipped() {
        // outdated コメント (line=None) はスキップされる
        let comments = vec![make_review_comment(
            "src/main.rs",
            None,
            "RIGHT",
            "Outdated comment",
        )];
        let app = TestAppBuilder::new()
            .with_custom_patch("@@ -0,0 +1 @@\n+line1", "added", 1, 0)
            .review_comments(comments)
            .build();
        let counts = app.existing_comment_counts();
        assert!(counts.is_empty());
    }

    #[test]
    fn test_existing_comment_counts_no_match() {
        // 別ファイルのコメントはマッチしない
        let comments = vec![make_review_comment(
            "other.rs",
            Some(1),
            "RIGHT",
            "Wrong file",
        )];
        let app = TestAppBuilder::new()
            .with_custom_patch("@@ -0,0 +1 @@\n+line1", "added", 1, 0)
            .review_comments(comments)
            .build();
        let counts = app.existing_comment_counts();
        assert!(counts.is_empty());
    }

    #[test]
    fn test_enter_opens_comment_view_on_comment_line() {
        let mut app = create_app_with_comments();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 2; // +line2 (コメントがある行)

        app.handle_normal_mode(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::CommentView);
        assert_eq!(app.review.viewing_comments.len(), 1);
        assert_eq!(app.review.viewing_comments[0].body, "Nice line!");
    }

    #[test]
    fn test_enter_does_not_open_comment_view_on_empty_line() {
        let mut app = create_app_with_comments();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 1; // +line1 (コメントがない行)

        app.handle_normal_mode(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.review.viewing_comments.is_empty());
    }

    #[test]
    fn test_comment_view_esc_closes() {
        let mut app = create_app_with_comments();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 2;

        // CommentView を開く
        app.handle_normal_mode(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::CommentView);

        // Esc で閉じる
        app.handle_comment_view_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.review.viewing_comments.is_empty());
    }

    /// 複数 hunk のパッチを持つ App を作成するヘルパー
    fn create_app_with_multi_hunk_patch() -> App {
        TestAppBuilder::new()
            .with_custom_patch(
                "@@ -1,3 +1,3 @@\n context\n-old line\n+new line\n@@ -10,3 +10,3 @@\n context2\n-old2\n+new2",
                "modified",
                2,
                2,
            )
            .build()
    }

    #[test]
    fn test_hunk_boundary_blocks_selection_down() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // カーソルを hunk1 の最後の行 (行3: "+new line") に移動
        app.diff.cursor_line = 3;
        app.enter_line_select_mode();

        // 行4 は @@ (hunk2 ヘッダー) → 別 hunk なので移動不可
        app.extend_selection_down();
        assert_eq!(app.diff.cursor_line, 3); // 移動しない
    }

    #[test]
    fn test_hunk_boundary_blocks_selection_up() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // カーソルを hunk2 の最初のコンテンツ行 (行5) に配置
        app.diff.cursor_line = 5;
        app.enter_line_select_mode();

        // 行4 は @@ ヘッダー → カーソル不可なので移動しない
        app.extend_selection_up();
        assert_eq!(app.diff.cursor_line, 5); // @@ 行にはカーソルを置けない
    }

    #[test]
    fn test_selection_within_same_hunk() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // hunk1 内 (行0) から選択開始
        app.diff.cursor_line = 0;
        app.enter_line_select_mode();

        // hunk1 内で自由に移動できる
        app.extend_selection_down(); // 行1
        assert_eq!(app.diff.cursor_line, 1);
        app.extend_selection_down(); // 行2
        assert_eq!(app.diff.cursor_line, 2);
        app.extend_selection_down(); // 行3
        assert_eq!(app.diff.cursor_line, 3);
        // 行4 (@@) は別 hunk → 停止
        app.extend_selection_down();
        assert_eq!(app.diff.cursor_line, 3);
    }

    #[test]
    fn test_is_same_hunk_within_hunk() {
        let app = create_app_with_multi_hunk_patch();
        // hunk1 内の行同士
        assert!(app.is_same_hunk(0, 1));
        assert!(app.is_same_hunk(0, 3));
        // hunk2 内の行同士
        assert!(app.is_same_hunk(4, 7));
        assert!(app.is_same_hunk(5, 6));
    }

    #[test]
    fn test_is_same_hunk_across_hunks() {
        let app = create_app_with_multi_hunk_patch();
        // hunk1 と hunk2 を跨ぐ
        assert!(!app.is_same_hunk(3, 4));
        assert!(!app.is_same_hunk(0, 5));
        assert!(!app.is_same_hunk(2, 7));
    }

    #[test]
    fn test_hunk_header_not_selectable_with_v() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // カーソルを @@ 行 (行0) に配置
        app.diff.cursor_line = 0;
        app.enter_line_select_mode();
        // @@ 行上では選択モードに入れない
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.line_selection.is_none());
    }

    #[test]
    fn test_hunk_header_not_selectable_with_c() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // カーソルを @@ 行 (行4) に配置
        app.diff.cursor_line = 4;
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);
        // @@ 行上ではコメント入力に入れない
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.line_selection.is_none());
    }

    #[test]
    fn test_page_down_moves_cursor_by_view_height() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.view_height = 3;
        app.diff.cursor_line = 0;

        app.page_down();
        assert_eq!(app.diff.cursor_line, 3);

        app.page_down();
        assert_eq!(app.diff.cursor_line, 6);
    }

    #[test]
    fn test_page_up_moves_cursor_by_view_height() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.view_height = 3;
        app.diff.cursor_line = 7;

        app.page_up();
        assert_eq!(app.diff.cursor_line, 4);

        app.page_up();
        assert_eq!(app.diff.cursor_line, 1);

        app.page_up();
        assert_eq!(app.diff.cursor_line, 0); // 0 で停止
    }

    #[test]
    fn test_ctrl_f_b_keybinds() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.view_height = 3;

        app.handle_normal_mode(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(app.diff.cursor_line, 3);

        app.handle_normal_mode(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(app.diff.cursor_line, 0);
    }

    #[test]
    fn test_jump_to_next_change() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // 行0: @@, 行1: context, 行2: -old, 行3: +new, 行4: @@, 行5: context2, 行6: -old2, 行7: +new2
        app.diff.cursor_line = 0;

        app.jump_to_next_change();
        assert_eq!(app.diff.cursor_line, 2); // ブロックA先頭 (-old line)

        app.jump_to_next_change();
        assert_eq!(app.diff.cursor_line, 6); // ブロックB先頭 (-old2)、ブロックA全体をスキップ

        // それ以降にブロックがないのでカーソルは動かない
        app.jump_to_next_change();
        assert_eq!(app.diff.cursor_line, 6);
    }

    #[test]
    fn test_jump_to_prev_change() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 7; // +new2 (ブロックB末尾)

        app.jump_to_prev_change();
        assert_eq!(app.diff.cursor_line, 6); // ブロックB先頭 (-old2)

        app.jump_to_prev_change();
        assert_eq!(app.diff.cursor_line, 2); // ブロックA先頭 (-old line)

        // それ以前にブロックがないのでカーソルは動かない
        app.jump_to_prev_change();
        assert_eq!(app.diff.cursor_line, 2);
    }

    #[test]
    fn test_jump_to_next_hunk() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 1; // 最初の hunk 内

        app.jump_to_next_hunk();
        assert_eq!(app.diff.cursor_line, 5); // 2番目の @@ の次の実コード行

        // それ以降に @@ がないのでカーソルは動かない
        app.jump_to_next_hunk();
        assert_eq!(app.diff.cursor_line, 5);
    }

    #[test]
    fn test_jump_to_prev_hunk() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 7; // 最終行

        app.jump_to_prev_hunk();
        assert_eq!(app.diff.cursor_line, 5); // 2番目の @@ の次の実コード行

        app.jump_to_prev_hunk();
        assert_eq!(app.diff.cursor_line, 1); // 最初の @@ の次の実コード行
    }

    #[test]
    fn test_two_key_sequence_bracket_c() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 0;

        // ]c → 次の変更行
        app.handle_normal_mode(KeyCode::Char(']'), KeyModifiers::NONE);
        assert!(app.pending_key.is_some());
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(app.pending_key.is_none());
        assert_eq!(app.diff.cursor_line, 2); // -old line

        // [c → 前の変更行
        app.diff.cursor_line = 7;
        app.handle_normal_mode(KeyCode::Char('['), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(app.diff.cursor_line, 6); // -old2
    }

    #[test]
    fn test_two_key_sequence_bracket_h() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 1;

        // ]h → 次の hunk の実コード行
        app.handle_normal_mode(KeyCode::Char(']'), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.diff.cursor_line, 5);

        // [h → 前の hunk の実コード行
        app.handle_normal_mode(KeyCode::Char('['), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.diff.cursor_line, 1);
    }

    #[test]
    fn test_two_key_sequence_invalid_second_key() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 0;

        // ]x → 不明な2文字目は無視、pending_key はクリアされる
        app.handle_normal_mode(KeyCode::Char(']'), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.pending_key.is_none());
        assert_eq!(app.diff.cursor_line, 0); // 動かない
    }

    #[test]
    fn test_jump_to_next_comment() {
        // patch: @@ -0,0 +1,5 @@\n+line1\n+line2\n+line3\n+line4\n+line5
        // idx:   0                 1       2       3       4       5
        // コメント: line 2 (idx 2), line 4 (idx 4)
        let comments = vec![
            make_review_comment("src/main.rs", Some(2), "RIGHT", "Comment A"),
            make_review_comment("src/main.rs", Some(4), "RIGHT", "Comment B"),
        ];
        let mut app = TestAppBuilder::new()
            .with_custom_patch(
                "@@ -0,0 +1,5 @@\n+line1\n+line2\n+line3\n+line4\n+line5",
                "added",
                5,
                0,
            )
            .review_comments(comments)
            .build();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 0;

        app.jump_to_next_comment();
        assert_eq!(app.diff.cursor_line, 2);

        app.jump_to_next_comment();
        assert_eq!(app.diff.cursor_line, 4);

        // それ以降にコメントがないのでカーソルは動かない
        app.jump_to_next_comment();
        assert_eq!(app.diff.cursor_line, 4);
    }

    #[test]
    fn test_jump_to_prev_comment() {
        let comments = vec![
            make_review_comment("src/main.rs", Some(2), "RIGHT", "Comment A"),
            make_review_comment("src/main.rs", Some(4), "RIGHT", "Comment B"),
        ];
        let mut app = TestAppBuilder::new()
            .with_custom_patch(
                "@@ -0,0 +1,5 @@\n+line1\n+line2\n+line3\n+line4\n+line5",
                "added",
                5,
                0,
            )
            .review_comments(comments)
            .build();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 5;

        app.jump_to_prev_comment();
        assert_eq!(app.diff.cursor_line, 4);

        app.jump_to_prev_comment();
        assert_eq!(app.diff.cursor_line, 2);

        // それ以前にコメントがないのでカーソルは動かない
        app.jump_to_prev_comment();
        assert_eq!(app.diff.cursor_line, 2);
    }

    #[test]
    fn test_jump_to_comment_no_comments() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 3;

        // コメントがない場合はカーソルが動かない
        app.jump_to_next_comment();
        assert_eq!(app.diff.cursor_line, 3);

        app.jump_to_prev_comment();
        assert_eq!(app.diff.cursor_line, 3);
    }

    #[test]
    fn test_two_key_sequence_bracket_n() {
        let comments = vec![make_review_comment(
            "src/main.rs",
            Some(2),
            "RIGHT",
            "Comment A",
        )];
        let mut app = TestAppBuilder::new()
            .with_custom_patch("@@ -0,0 +1,3 @@\n+line1\n+line2\n+line3", "added", 3, 0)
            .review_comments(comments)
            .build();
        app.focused_panel = Panel::DiffView;
        app.diff.cursor_line = 0;

        // ]n → 次のコメント行
        app.handle_normal_mode(KeyCode::Char(']'), KeyModifiers::NONE);
        assert!(app.pending_key.is_some());
        app.handle_normal_mode(KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(app.pending_key.is_none());
        assert_eq!(app.diff.cursor_line, 2);

        // [n → 前のコメント行（ここでは先頭方向にコメントがないので動かない）
        app.handle_normal_mode(KeyCode::Char('['), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(app.diff.cursor_line, 2);
    }

    // === N12: Zoom モードテスト ===

    #[test]
    fn test_zoom_toggle() {
        let mut app = TestAppBuilder::new().with_test_data().build();

        assert!(!app.zoomed);

        // z キーで zoom on
        app.handle_normal_mode(KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(app.zoomed);

        // もう一度 z で zoom off
        app.handle_normal_mode(KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(!app.zoomed);
    }

    #[test]
    fn test_zoom_works_in_all_panels() {
        let mut app = TestAppBuilder::new().with_test_data().build();

        // 各ペインで zoom できる
        for panel in [
            Panel::PrDescription,
            Panel::CommitList,
            Panel::FileTree,
            Panel::DiffView,
        ] {
            app.focused_panel = panel;
            app.zoomed = false;
            app.handle_normal_mode(KeyCode::Char('z'), KeyModifiers::NONE);
            assert!(app.zoomed, "zoom should work in {:?}", panel);
        }
    }

    #[test]
    fn test_zoom_panel_navigation() {
        let mut app = TestAppBuilder::new().with_test_data().build();

        app.zoomed = true;
        app.focused_panel = Panel::PrDescription;

        // zoom 中もペイン切り替えは可能（Tab で次のペインへ）
        app.handle_normal_mode(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::CommitList);
        assert!(app.zoomed); // zoom は維持
    }

    // === N13: Hunk ヘッダーデザインテスト ===

    #[test]
    fn test_format_hunk_header_basic() {
        let line = App::format_hunk_header("@@ -10,5 +12,7 @@ fn main()", 40, Style::default());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with("─── L10-14 → L12-18 ─── fn main() "));
        // 幅40まで ─ で埋められている
        assert!(text.ends_with('─'));
    }

    #[test]
    fn test_format_hunk_header_no_context() {
        let line = App::format_hunk_header("@@ -1,3 +1,3 @@", 30, Style::default());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with("─── L1-3 → L1-3 "));
        // コンテキストなし → range の後にすぐ ─ 埋め
        assert!(!text.contains("fn "));
    }

    #[test]
    fn test_format_hunk_header_single_line() {
        // len=1 のとき（カンマなし）→ L10 のように表示
        let line = App::format_hunk_header("@@ -10 +12,3 @@", 30, Style::default());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with("─── L10 → L12-14 "));
    }

    #[test]
    fn test_format_hunk_header_new_file() {
        // 新規ファイル: @@ -0,0 +1,5 @@
        let line = App::format_hunk_header("@@ -0,0 +1,5 @@", 30, Style::default());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("L1-5"));
    }

    #[test]
    fn test_format_hunk_header_long_context_truncated() {
        // 関数名が非常に長い場合、width に収まるようトランケートされる
        let long_ctx = format!(
            "@@ -1,3 +1,3 @@ {}",
            "a_very_long_function_name_that_exceeds_width"
        );
        let line = App::format_hunk_header(&long_ctx, 30, Style::default());
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // 幅30を超えない
        assert!(UnicodeWidthStr::width(text.as_str()) <= 30);
        // 末尾は ─ で終わる
        assert!(text.ends_with('─'));
    }

    #[test]
    fn test_truncate_path_no_truncation() {
        assert_eq!(truncate_path("src/main.rs", 20), "src/main.rs");
    }

    #[test]
    fn test_truncate_path_exact_width() {
        assert_eq!(truncate_path("src/main.rs", 11), "src/main.rs");
    }

    #[test]
    fn test_truncate_path_with_slash() {
        let result = truncate_path("src/components/MyComponent/index.tsx", 20);
        assert!(result.starts_with("..."));
        assert!(result.len() <= 20);
        assert!(result.contains("/"));
    }

    #[test]
    fn test_truncate_path_without_slash_in_tail() {
        // tail 部分に '/' がない場合はそのまま "...tail"
        let result = truncate_path("abcdefghij", 8);
        assert_eq!(result, "...fghij");
    }

    #[test]
    fn test_truncate_path_small_width() {
        assert_eq!(truncate_path("src/main.rs", 3), "src");
        assert_eq!(truncate_path("src/main.rs", 2), "sr");
        assert_eq!(truncate_path("src/main.rs", 1), "s");
        assert_eq!(truncate_path("src/main.rs", 0), "");
    }

    #[test]
    fn test_truncate_str_no_truncation() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_truncated() {
        assert_eq!(truncate_str("hello world", 6), "hello…");
        assert_eq!(truncate_str("hello world", 2), "h…");
    }

    #[test]
    fn test_truncate_str_zero_and_one() {
        assert_eq!(truncate_str("hello", 0), "");
        assert_eq!(truncate_str("hello", 1), "…");
    }

    #[test]
    fn test_truncate_str_cjk() {
        // CJK文字は幅2。"日本語" = 幅6
        assert_eq!(truncate_str("日本語", 6), "日本語");
        assert_eq!(truncate_str("日本語", 5), "日本…");
        assert_eq!(truncate_str("日本語", 3), "日…");
    }

    #[test]
    fn test_whitespace_only_lines_cleared_for_wrap() {
        // 空白のみの行に対するクリア処理が安全に動作することを検証する
        use ratatui::text::Line as RLine;
        use ratatui::widgets::{Paragraph, Wrap};

        // ratatui 0.30 では空白1文字の Line も wrap で正しく line_count 1 を返す
        let count_space = Paragraph::new(RLine::raw(" "))
            .wrap(Wrap { trim: false })
            .line_count(80);
        assert_eq!(count_space, 1);

        // spans が空の Line でも line_count は正しく 1 を返す
        let count_default = Paragraph::new(RLine::default())
            .wrap(Wrap { trim: false })
            .line_count(80);
        assert_eq!(count_default, 1);

        // クリア処理を適用しても line_count は変わらない（安全であることを検証）
        let mut line = RLine::raw(" ");
        let all_whitespace = line.spans.iter().all(|s| s.content.trim().is_empty());
        assert!(all_whitespace);
        line.spans.clear();
        let count_cleared = Paragraph::new(line)
            .wrap(Wrap { trim: false })
            .line_count(80);
        assert_eq!(count_cleared, 1);
    }

    // キャッシュされた表示行オフセットから論理行の開始位置を正しく返すことを検証
    #[test]
    fn test_visual_line_offset_with_cache() {
        let mut app = TestAppBuilder::new().build();
        app.diff.wrap = true;
        // line 0 → row 0, line 1 → row 1, line 2 → row 3, line 3 → row 4, total → 7
        app.diff.visual_offsets = Some(vec![0, 1, 3, 4, 7]);

        assert_eq!(app.visual_line_offset(0), 0);
        assert_eq!(app.visual_line_offset(1), 1);
        assert_eq!(app.visual_line_offset(2), 3);
        assert_eq!(app.visual_line_offset(3), 4);
        assert_eq!(app.visual_line_offset(4), 7); // 合計表示行数
    }

    // キャッシュから表示行→論理行の逆引きが正しく行われることを検証
    #[test]
    fn test_visual_to_logical_line_with_cache() {
        let mut app = TestAppBuilder::new().build();
        app.diff.wrap = true;
        // line 0 → row 0, line 1 → rows 1-2, line 2 → row 3, line 3 → rows 4-6, total → 7
        app.diff.visual_offsets = Some(vec![0, 1, 3, 4, 7]);

        assert_eq!(app.visual_to_logical_line(0), 0);
        assert_eq!(app.visual_to_logical_line(1), 1);
        assert_eq!(app.visual_to_logical_line(2), 1); // row 2 は line 1 の折り返し部分
        assert_eq!(app.visual_to_logical_line(3), 2);
        assert_eq!(app.visual_to_logical_line(4), 3);
        assert_eq!(app.visual_to_logical_line(5), 3); // row 5 は line 3 の折り返し部分
        assert_eq!(app.visual_to_logical_line(6), 3); // row 6 も line 3 の一部
    }

    // wrap 無効時は論理行＝表示行としてそのまま返すことを検証
    #[test]
    fn test_visual_line_offset_no_wrap() {
        let app = TestAppBuilder::new().build();
        // diff_wrap はデフォルトで false

        assert_eq!(app.visual_line_offset(0), 0);
        assert_eq!(app.visual_line_offset(5), 5);
        assert_eq!(app.visual_to_logical_line(5), 5);
    }

    /// 長い行を含むパッチで wrap + 行番号の visual_line_offset を検証
    #[test]
    fn test_visual_line_offset_with_line_numbers() {
        let mut files_map = HashMap::new();
        let long_line = format!("+{}", "x".repeat(120));
        let patch = format!("@@ -1,3 +1,3 @@\n context\n-old\n{}", long_line);
        files_map.insert(
            TEST_SHA_0.to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "modified".to_string(),
                additions: 1,
                deletions: 1,
                patch: Some(patch),
            }],
        );
        let mut app = TestAppBuilder::new()
            .with_commits()
            .files_map(files_map)
            .build();
        app.diff.view_width = 80;
        app.diff.wrap = true;
        app.diff.show_line_numbers = true;

        let with_numbers = app.visual_line_offset(4);
        assert!(
            with_numbers > 4,
            "行番号ONで長い行は wrap により視覚行数が論理行数より多い"
        );

        app.diff.show_line_numbers = false;
        let without_numbers = app.visual_line_offset(4);
        assert!(
            with_numbers >= without_numbers,
            "行番号ONは行番号OFFより視覚行数が多い（もしくは同じ）"
        );
    }

    /// wrap + 行番号で ensure_cursor_visible がカーソルを画面内に収める
    #[test]
    fn test_ensure_cursor_visible_with_wrap_and_line_numbers() {
        let mut files_map = HashMap::new();
        let lines: Vec<String> = (0..20)
            .map(|i| format!("+{}", format!("line{} ", i).repeat(20)))
            .collect();
        let patch = format!("@@ -0,0 +1,20 @@\n{}", lines.join("\n"));
        files_map.insert(
            TEST_SHA_0.to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "added".to_string(),
                additions: 20,
                deletions: 0,
                patch: Some(patch),
            }],
        );
        let mut app = TestAppBuilder::new()
            .with_commits()
            .files_map(files_map)
            .build();
        app.diff.view_width = 80;
        app.diff.view_height = 10;
        app.diff.wrap = true;
        app.diff.show_line_numbers = true;
        app.focused_panel = Panel::DiffView;

        app.diff.cursor_line = 20;
        app.ensure_cursor_visible();

        let cursor_visual = app.visual_line_offset(app.diff.cursor_line);
        let cursor_visual_end = app.visual_line_offset(app.diff.cursor_line + 1);
        let scroll = app.diff.scroll as usize;
        let visible = app.diff.view_height as usize;

        assert!(
            cursor_visual >= scroll,
            "カーソルの先頭がスクロール位置より下にある: cursor_visual={}, scroll={}",
            cursor_visual,
            scroll
        );
        assert!(
            cursor_visual_end <= scroll + visible,
            "カーソルの末尾が画面内に収まっている: cursor_visual_end={}, scroll+visible={}",
            cursor_visual_end,
            scroll + visible
        );
    }

    /// line_number_prefix_width が file_status に応じた正しい幅を返す
    #[test]
    fn test_line_number_prefix_width() {
        // modified ファイル → 両カラム 11文字
        let mut app = TestAppBuilder::new()
            .with_custom_patch("@@ -1 +1 @@\n-old\n+new", "modified", 1, 1)
            .build();
        app.diff.show_line_numbers = true;
        assert_eq!(app.line_number_prefix_width(), 11);

        // added ファイル → 片カラム 6文字
        let mut files_map = HashMap::new();
        files_map.insert(
            TEST_SHA_0.to_string(),
            vec![DiffFile {
                filename: "src/new.rs".to_string(),
                status: "added".to_string(),
                additions: 1,
                deletions: 0,
                patch: Some("@@ -0,0 +1 @@\n+new".to_string()),
            }],
        );
        let mut app = TestAppBuilder::new()
            .with_commits()
            .files_map(files_map)
            .build();
        app.diff.show_line_numbers = true;
        assert_eq!(app.line_number_prefix_width(), 6);

        // 行番号OFF → 0文字
        app.diff.show_line_numbers = false;
        assert_eq!(app.line_number_prefix_width(), 0);
    }

    #[test]
    fn test_preprocess_pr_body_markdown_image() {
        let body = "Some text\n![screenshot](https://github.com/user-attachments/assets/abc123)\nMore text";
        let (result, refs) = preprocess_pr_body(body);
        assert!(result.contains("[🖼 screenshot]"));
        assert!(!result.contains("![screenshot]"));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].media_type, MediaType::Image);
        assert_eq!(refs[0].alt, "screenshot");
    }

    #[test]
    fn test_preprocess_pr_body_html_img() {
        let body =
            "Before\n<img src=\"https://github.com/user-attachments/assets/abc123\" />\nAfter";
        let (result, refs) = preprocess_pr_body(body);
        assert!(result.contains("[🖼 Image]"));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].media_type, MediaType::Image);
    }

    #[test]
    fn test_preprocess_pr_body_video_bare_url() {
        let body = "Check this:\nhttps://github.com/user-attachments/assets/abc123.mp4\nEnd";
        let (result, refs) = preprocess_pr_body(body);
        assert!(result.contains("[🎬 Video]"));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].media_type, MediaType::Video);
    }

    #[test]
    fn test_preprocess_pr_body_video_bare_uuid_url() {
        // GitHub user-attachments の動画 URL は拡張子なし（UUID のみ）の場合がある
        let body = "Summary\nhttps://github.com/user-attachments/assets/997a4417-2117-4a04-83ab-bcd341df33d3\nEnd";
        let (result, refs) = preprocess_pr_body(body);
        assert!(result.contains("[🎬 Video]"));
        assert!(!result.contains("997a4417"));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].media_type, MediaType::Video);
    }

    #[test]
    fn test_preprocess_pr_body_video_bare_private_user_images_url() {
        // private-user-images URL も拡張子なしでベア URL の場合は動画と推定する
        let body = "Summary\nhttps://private-user-images.githubusercontent.com/12345/997a4417-2117-4a04-83ab-bcd341df33d3?jwt=abc\nEnd";
        let (result, refs) = preprocess_pr_body(body);
        assert!(result.contains("[🎬 Video]"));
        assert!(!result.contains("997a4417"));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].media_type, MediaType::Video);
    }

    #[test]
    fn test_preprocess_pr_body_html_video() {
        let body = "<video src=\"https://github.com/user-attachments/assets/abc.mov\"></video>";
        let (result, refs) = preprocess_pr_body(body);
        assert!(result.contains("[🎬 Video]"));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].media_type, MediaType::Video);
    }

    #[test]
    fn test_process_inline_media_with_multibyte_characters() {
        let line = "日本語テキスト![画像](https://example.com/img.png)の後も日本語";
        let mut refs = Vec::new();
        let mut result_lines = Vec::new();
        let matched = process_inline_media(line, &mut refs, &mut result_lines);
        assert!(matched);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].alt, "画像");
        assert!(result_lines.iter().any(|l| l.contains("日本語テキスト")));
        assert!(result_lines.iter().any(|l| l.contains("の後も日本語")));
    }

    #[test]
    fn test_process_inline_media_multibyte_only() {
        let line = "日本語だけのテキスト、画像なし";
        let mut refs = Vec::new();
        let mut result_lines = Vec::new();
        let matched = process_inline_media(line, &mut refs, &mut result_lines);
        assert!(!matched);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_process_inline_media_html_img_with_japanese() {
        let line = "前文<img src=\"https://example.com/img.png\" alt=\"日本語alt\">後文";
        let mut refs = Vec::new();
        let mut result_lines = Vec::new();
        let matched = process_inline_media(line, &mut refs, &mut result_lines);
        assert!(matched);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].alt, "日本語alt");
    }

    #[test]
    fn test_preprocess_pr_body_no_media() {
        let body = "Just plain text\nwith no images";
        let (result, refs) = preprocess_pr_body(body);
        assert_eq!(result, body);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_preprocess_pr_body_multiple_media() {
        let body = "![img1](https://github.com/user-attachments/assets/a)\nText\n![img2](https://github.com/user-attachments/assets/b)";
        let (_, refs) = preprocess_pr_body(body);
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn test_preprocess_pr_body_img_with_alt() {
        let body = r#"<img src="https://example.com/img.png" alt="My Alt" />"#;
        let (result, refs) = preprocess_pr_body(body);
        assert!(result.contains("[🖼 My Alt]"));
        assert_eq!(refs[0].alt, "My Alt");
    }

    #[test]
    fn test_collect_image_urls_markdown_image() {
        let body = "Some text\n![screenshot](https://example.com/img.png)\nMore text";
        let urls = collect_image_urls(body);
        assert_eq!(urls, vec!["https://example.com/img.png"]);
    }

    #[test]
    fn test_collect_image_urls_html_img() {
        let body = r#"Before<img src="https://example.com/photo.jpg" alt="alt" />After"#;
        let urls = collect_image_urls(body);
        assert_eq!(urls, vec!["https://example.com/photo.jpg"]);
    }

    #[test]
    fn test_collect_image_urls_multiple() {
        let body = "![a](https://example.com/1.png)\nText\n![b](https://example.com/2.png)";
        let urls = collect_image_urls(body);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com/1.png");
        assert_eq!(urls[1], "https://example.com/2.png");
    }

    #[test]
    fn test_collect_image_urls_ignores_video() {
        // 動画 URL（ベア URL や <video> タグ）は収集しない
        let body = "https://github.com/user-attachments/assets/abc123.mp4\n<video src=\"https://example.com/v.mov\"></video>";
        let urls = collect_image_urls(body);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_collect_image_urls_no_media() {
        let body = "Just plain text\nwith no images";
        let urls = collect_image_urls(body);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_review_body_input_typing() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        app.review.review_event_cursor = 1; // Approve

        // 文字入力
        app.handle_review_body_input_mode(KeyCode::Char('L'), KeyModifiers::NONE);
        app.handle_review_body_input_mode(KeyCode::Char('G'), KeyModifiers::NONE);
        app.handle_review_body_input_mode(KeyCode::Char('T'), KeyModifiers::NONE);
        app.handle_review_body_input_mode(KeyCode::Char('M'), KeyModifiers::NONE);
        assert_eq!(app.review.review_body_editor.text(), "LGTM");

        // Backspace
        app.handle_review_body_input_mode(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.review.review_body_editor.text(), "LGT");
    }

    #[test]
    fn test_review_body_input_ctrl_s_submits() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        app.review.review_event_cursor = 1; // Approve
        for ch in "LGTM!".chars() {
            app.review.review_body_editor.insert_char(ch);
        }

        app.handle_review_body_input_mode(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.review.needs_submit, Some(ReviewEvent::Approve));
    }

    #[test]
    fn test_review_body_input_empty_body_submits() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        app.review.review_event_cursor = 1; // Approve

        // 空bodyでも Ctrl+S で送信可能
        app.handle_review_body_input_mode(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.review.needs_submit, Some(ReviewEvent::Approve));
    }

    #[test]
    fn test_review_body_input_esc_returns_to_submit() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        for ch in "some text".chars() {
            app.review.review_body_editor.insert_char(ch);
        }

        app.handle_review_body_input_mode(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::ReviewSubmit);
        assert!(app.review.review_body_editor.is_empty());
        assert!(app.review.needs_submit.is_none());
    }

    #[test]
    fn test_review_body_input_esc_preserves_quit_after_submit() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        app.review.quit_after_submit = true;

        // Esc で ReviewSubmit に戻る（quit_after_submit はリセットしない）
        app.handle_review_body_input_mode(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::ReviewSubmit);
        assert!(app.review.quit_after_submit);
    }

    // --- is_own_pr テスト ---

    fn create_own_pr_app() -> App {
        TestAppBuilder::new()
            .with_custom_patch("+line1", "added", 1, 0)
            .own_pr()
            .build()
    }

    #[test]
    fn test_own_pr_available_events_comment_only() {
        let app = create_own_pr_app();
        let events = app.available_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ReviewEvent::Comment);
    }

    #[test]
    fn test_not_own_pr_available_events_all() {
        let app = create_app_with_patch();
        let events = app.available_events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], ReviewEvent::Comment);
        assert_eq!(events[1], ReviewEvent::Approve);
        assert_eq!(events[2], ReviewEvent::RequestChanges);
    }

    #[test]
    fn test_own_pr_review_submit_cursor_stays_zero() {
        let mut app = create_own_pr_app();
        app.mode = AppMode::ReviewSubmit;

        // j/k で循環しても要素1つなのでカーソルは0のまま
        app.handle_review_submit_mode(KeyCode::Char('j'));
        assert_eq!(app.review.review_event_cursor, 0);
        app.handle_review_submit_mode(KeyCode::Char('k'));
        assert_eq!(app.review.review_event_cursor, 0);
        app.handle_review_submit_mode(KeyCode::Down);
        assert_eq!(app.review.review_event_cursor, 0);
        app.handle_review_submit_mode(KeyCode::Up);
        assert_eq!(app.review.review_event_cursor, 0);
    }

    /// Paragraph::line_count は block 付きだとボーダー行を含む値を返す。
    /// そのため line_count は block なしの Paragraph で呼ぶ必要がある。
    #[test]
    fn test_paragraph_line_count_block_inflates() {
        use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

        let text = "line1\nline2\nline3\nline4";
        let inner_width: u16 = 78;

        // block なし: 純粋なテキスト行数
        let count_no_block = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .line_count(inner_width);
        assert_eq!(count_no_block, 4);

        // block あり: ボーダー行が加算される
        let count_with_block = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false })
            .line_count(inner_width);
        assert_eq!(count_with_block, 6, "block adds 2 border lines");

        // スクロール計算には block なしの値を使うべき
        let view_height: u16 = 4;
        let max_scroll_correct = (count_no_block as u16).saturating_sub(view_height);
        assert_eq!(
            max_scroll_correct, 0,
            "4 lines fit in 4-line view, no scroll needed"
        );

        let max_scroll_wrong = (count_with_block as u16).saturating_sub(view_height);
        assert_eq!(
            max_scroll_wrong, 2,
            "block-inflated count wrongly allows 2 lines of scroll"
        );
    }

    // ── Issue Comment Input モード ──────────────────────────────

    #[test]
    fn test_conversation_c_key_enters_issue_comment_input() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::Conversation;

        // 'c' キーで IssueCommentInput モードに遷移
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::IssueCommentInput);
        assert!(app.review.comment_editor.is_empty());
    }

    #[test]
    fn test_issue_comment_input_esc_cancels() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::Conversation;
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::IssueCommentInput);

        // テキスト入力後に Esc → エディタクリア、Normal モード、Conversation パネル
        app.handle_issue_comment_input_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(!app.review.comment_editor.is_empty());

        app.handle_issue_comment_input_mode(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.focused_panel, Panel::Conversation);
        assert!(app.review.comment_editor.is_empty());
    }

    #[test]
    fn test_issue_comment_input_ctrl_s_empty_shows_error() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::Conversation;
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);

        // 空テキストで Ctrl+S → エラーメッセージ、フラグは false
        app.handle_issue_comment_input_mode(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert!(!app.needs_issue_comment_submit);
        assert!(app.status_message.is_some());
        assert_eq!(
            app.status_message.as_ref().unwrap().level,
            StatusLevel::Error
        );
    }

    #[test]
    fn test_issue_comment_input_ctrl_s_with_text_sets_flag() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::Conversation;
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);

        // テキスト入力
        app.handle_issue_comment_input_mode(KeyCode::Char('H'), KeyModifiers::NONE);
        app.handle_issue_comment_input_mode(KeyCode::Char('i'), KeyModifiers::NONE);

        // Ctrl+S → フラグ設定、Normal モード、Conversation パネル
        app.handle_issue_comment_input_mode(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert!(app.needs_issue_comment_submit);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.focused_panel, Panel::Conversation);
    }

    #[test]
    fn test_issue_comment_input_typing() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::Conversation;
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);

        // 文字入力がエディタに反映される
        app.handle_issue_comment_input_mode(KeyCode::Char('A'), KeyModifiers::NONE);
        app.handle_issue_comment_input_mode(KeyCode::Char('B'), KeyModifiers::NONE);
        assert_eq!(app.review.comment_editor.text(), "AB");

        // Backspace
        app.handle_issue_comment_input_mode(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.review.comment_editor.text(), "A");
    }

    #[test]
    fn test_submit_issue_comment_without_client_sets_error() {
        let mut app = create_app_with_patch();
        // client は None（テストデフォルト）
        app.review
            .comment_editor
            .handle_key(KeyCode::Char('x'), KeyModifiers::NONE);

        app.submit_issue_comment();
        assert!(app.status_message.is_some());
        assert_eq!(
            app.status_message.as_ref().unwrap().level,
            StatusLevel::Error
        );
    }

    #[test]
    fn test_blocking_operation_message_none_by_default() {
        let app = TestAppBuilder::new().build();
        assert!(app.blocking_operation_message().is_none());
    }

    #[test]
    fn test_blocking_operation_message_reload() {
        let mut app = TestAppBuilder::new().build();
        app.needs_reload = true;
        assert_eq!(
            app.blocking_operation_message(),
            Some("Reloading PR data...")
        );
    }

    #[test]
    fn test_blocking_operation_message_submit_review() {
        let mut app = TestAppBuilder::new().build();
        app.review.needs_submit = Some(ReviewEvent::Comment);
        assert_eq!(
            app.blocking_operation_message(),
            Some("Submitting review...")
        );
    }

    #[test]
    fn test_blocking_operation_message_issue_comment() {
        let mut app = TestAppBuilder::new().build();
        app.needs_issue_comment_submit = true;
        assert_eq!(
            app.blocking_operation_message(),
            Some("Submitting comment...")
        );
    }

    #[test]
    fn test_blocking_operation_message_reply() {
        let mut app = TestAppBuilder::new().build();
        app.needs_reply_submit = true;
        assert_eq!(
            app.blocking_operation_message(),
            Some("Submitting reply...")
        );
    }

    #[test]
    fn test_blocking_operation_message_resolve_toggle() {
        let mut app = TestAppBuilder::new().build();
        app.review.needs_resolve_toggle = Some(ResolveToggleRequest {
            thread_node_id: "test".to_string(),
            should_resolve: true,
            root_comment_id: 1,
        });
        assert_eq!(app.blocking_operation_message(), Some("Updating thread..."));
    }
}
