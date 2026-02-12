mod handler;
mod navigation;
mod render;
mod style;
mod types;

use style::PrDescStyleSheet;
pub use types::*;

use crate::github::comments::ReviewComment;
use crate::github::commits::CommitInfo;
use crate::github::files::DiffFile;
use crate::github::media::MediaCache;
use crate::github::review::{self, PendingComment};
use color_eyre::Result;
use octocrab::Octocrab;
use ratatui::{
    DefaultTerminal,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::ListState,
};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::{HashMap, HashSet};
use tokio::runtime::Handle;
use unicode_width::UnicodeWidthStr;

/// PR body から画像 URL のみを軽量に収集する。
/// `preprocess_pr_body` と異なり、テキスト置換は行わない。
/// 対象パターン: `![alt](url)` および `<img src="url" ...>`
pub fn collect_image_urls(body: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for line in body.lines() {
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            // Markdown image: ![alt](url)
            if bytes[pos] == b'!'
                && pos + 1 < bytes.len()
                && bytes[pos + 1] == b'['
                && let Some((_alt, url, end)) = parse_markdown_image(line, pos)
            {
                urls.push(url);
                pos = end;
                continue;
            }
            // HTML <img> tag
            if bytes[pos] == b'<' {
                let rest = &line[pos..];
                let lower_rest = rest.to_lowercase();
                if (lower_rest.starts_with("<img ") || lower_rest.starts_with("<img>"))
                    && let Some((_alt, url, end_offset)) = parse_html_img(rest)
                {
                    urls.push(url);
                    pos += end_offset;
                    continue;
                }
            }
            pos += 1;
        }
    }
    urls
}

/// PR body 中のメディア参照を検出し、プレースホルダーに置換する。
/// 戻り値: (置換済みテキスト, 検出されたメディア一覧)
pub fn preprocess_pr_body(body: &str) -> (String, Vec<MediaRef>) {
    let mut refs: Vec<MediaRef> = Vec::new();
    let mut result_lines: Vec<String> = Vec::new();

    for line in body.lines() {
        let trimmed = line.trim();

        // --- Pattern 4: HTML <video> tag ---
        if let Some(processed) = try_parse_html_video(trimmed) {
            result_lines.push(String::new());
            result_lines.push("[🎬 Video]".to_string());
            result_lines.push(String::new());
            refs.push(MediaRef {
                media_type: MediaType::Video,
                url: processed,
                alt: "Video".to_string(),
            });
            continue;
        }

        // --- Pattern 3: Bare video URL on its own line ---
        if let Some(url) = try_parse_bare_video_url(trimmed) {
            result_lines.push(String::new());
            result_lines.push("[🎬 Video]".to_string());
            result_lines.push(String::new());
            refs.push(MediaRef {
                media_type: MediaType::Video,
                url,
                alt: "Video".to_string(),
            });
            continue;
        }

        // --- Pattern 2: HTML <img> tag ---
        // --- Pattern 1: Markdown image ![alt](url) ---
        // These can appear inline, so we process within the line
        let processed = process_inline_media(line, &mut refs, &mut result_lines);
        if !processed {
            result_lines.push(line.to_string());
        }
    }

    // 前後の空行の重複を除去する
    let output = collapse_blank_lines(&result_lines);
    (output, refs)
}

/// 連続する空行を最大1つに縮小する
fn collapse_blank_lines(lines: &[String]) -> String {
    let mut result = String::new();
    let mut prev_blank = false;
    for (i, line) in lines.iter().enumerate() {
        let is_blank = line.trim().is_empty();
        if is_blank && prev_blank {
            continue;
        }
        if i > 0 {
            result.push('\n');
        }
        result.push_str(line);
        prev_blank = is_blank;
    }
    result
}

/// HTML <video> タグを検出し、src URL を返す
fn try_parse_html_video(line: &str) -> Option<String> {
    // <video で始まるかチェック
    let lower = line.to_lowercase();
    if !lower.contains("<video") {
        return None;
    }
    // src="..." を抽出
    extract_html_attr(line, "src")
}

/// 行が動画ベア URL かどうかチェック。
/// GitHub user-attachments URL は拡張子なし（UUID のみ）の場合がある。
/// Markdown 画像 `![](url)` でラップされていないベア URL は動画と推定する。
fn try_parse_bare_video_url(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let is_asset_url = trimmed.starts_with("https://github.com/user-attachments/assets/")
        || trimmed.starts_with("https://private-user-images.githubusercontent.com/");
    if !is_asset_url {
        return None;
    }
    // 明示的な動画拡張子があれば動画確定
    let url_path = trimmed.split('?').next().unwrap_or(trimmed);
    if url_path.ends_with(".mp4") || url_path.ends_with(".mov") || url_path.ends_with(".webm") {
        return Some(trimmed.to_string());
    }
    // 拡張子なしのアセット URL がベア URL として出現する場合、
    // 動画の可能性が高い（画像は通常 ![alt](url) でラップされるため）
    Some(trimmed.to_string())
}

/// 行内の Markdown 画像と HTML img タグを処理する。
/// 置換が発生した場合は true を返し、result_lines に追加済み。
fn process_inline_media(
    line: &str,
    refs: &mut Vec<MediaRef>,
    result_lines: &mut Vec<String>,
) -> bool {
    let mut replaced = String::new();
    let mut had_match = false;
    let mut pos = 0;
    let bytes = line.as_bytes();

    while pos < bytes.len() {
        // Try Markdown image: ![alt](url)
        if bytes[pos] == b'!'
            && pos + 1 < bytes.len()
            && bytes[pos + 1] == b'['
            && let Some((alt, url, end)) = parse_markdown_image(line, pos)
        {
            had_match = true;
            let display_alt = if alt.is_empty() {
                "Image".to_string()
            } else {
                alt.clone()
            };
            // 前のテキストがあれば先に追加
            if !replaced.is_empty() {
                result_lines.push(replaced.clone());
                replaced.clear();
            }
            result_lines.push(String::new());
            result_lines.push(format!("[🖼 {}]", display_alt));
            result_lines.push(String::new());
            refs.push(MediaRef {
                media_type: MediaType::Image,
                url,
                alt: display_alt,
            });
            pos = end;
            continue;
        }

        // Try HTML <img> tag
        if bytes[pos] == b'<' {
            let rest = &line[pos..];
            let lower_rest = rest.to_lowercase();
            if (lower_rest.starts_with("<img ") || lower_rest.starts_with("<img>"))
                && let Some((alt, url, end_offset)) = parse_html_img(rest)
            {
                had_match = true;
                let display_alt = if alt.is_empty() {
                    "Image".to_string()
                } else {
                    alt
                };
                if !replaced.is_empty() {
                    result_lines.push(replaced.clone());
                    replaced.clear();
                }
                result_lines.push(String::new());
                result_lines.push(format!("[🖼 {}]", display_alt));
                result_lines.push(String::new());
                refs.push(MediaRef {
                    media_type: MediaType::Image,
                    url,
                    alt: display_alt,
                });
                pos += end_offset;
                continue;
            }
        }

        // マルチバイト文字に対応するため、文字単位で処理する
        let ch = line[pos..].chars().next().unwrap();
        replaced.push(ch);
        pos += ch.len_utf8();
    }

    if had_match {
        // 残りのテキストがあれば追加
        let trimmed = replaced.trim();
        if !trimmed.is_empty() {
            result_lines.push(replaced);
        }
        true
    } else {
        false
    }
}

/// Markdown 画像 `![alt](url)` をパース。成功時は (alt, url, end_pos) を返す。
fn parse_markdown_image(line: &str, start: usize) -> Option<(String, String, usize)> {
    // start は '!' の位置、start+1 は '['
    let after_bang = start + 2; // '[' の次
    let alt_end = line[after_bang..].find(']')?;
    let alt = &line[after_bang..after_bang + alt_end];

    let paren_start = after_bang + alt_end + 1; // ']' の次
    if paren_start >= line.len() || line.as_bytes()[paren_start] != b'(' {
        return None;
    }
    let url_start = paren_start + 1;
    let paren_end = line[url_start..].find(')')?;
    let url = &line[url_start..url_start + paren_end];

    Some((alt.to_string(), url.to_string(), url_start + paren_end + 1))
}

/// HTML <img ...> タグをパース。成功時は (alt, src_url, end_offset) を返す。
/// end_offset は入力文字列の先頭からの相対位置。
fn parse_html_img(tag_str: &str) -> Option<(String, String, usize)> {
    // タグの終端を探す: "/>" or ">"
    let end_pos = find_tag_end(tag_str)?;
    let tag_content = &tag_str[..end_pos];

    let src = extract_html_attr(tag_content, "src")?;
    let alt = extract_html_attr(tag_content, "alt").unwrap_or_default();

    Some((alt, src, end_pos))
}

/// HTML タグ文字列の終端位置を探す（`/>` or `>` の直後）
fn find_tag_end(s: &str) -> Option<usize> {
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
            return Some(i + 2);
        }
        if bytes[i] == b'>' {
            // </video> のような閉じタグも考慮
            // タグ全体の終わりを返す
            // <video ...>...</video> パターンの場合
            let rest = &s[i + 1..];
            let lower_rest = rest.to_lowercase();
            if let Some(close_pos) = lower_rest.find("</video>") {
                return Some(i + 1 + close_pos + 8); // 8 = "</video>".len()
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// HTML 属性値を抽出（例: `src="value"` → `value`）
fn extract_html_attr(tag: &str, attr_name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let search = format!("{}=\"", attr_name);
    let idx = lower.find(&search)?;
    let value_start = idx + search.len();
    let rest = &tag[value_start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

pub struct App {
    should_quit: bool,
    focused_panel: Panel,
    mode: AppMode,
    pr_number: u64,
    repo: String,
    pr_title: String,
    pr_body: String,
    pr_author: String,
    commits: Vec<CommitInfo>,
    commit_list_state: ListState,
    files_map: HashMap<String, Vec<DiffFile>>,
    file_list_state: ListState,
    pr_desc_scroll: u16,
    /// PR Description ペインの表示可能行数（render 時に更新）
    pr_desc_view_height: u16,
    /// PR Description の Wrap 考慮済み視覚行数（render 時に更新）
    pr_desc_visual_total: u16,
    diff_scroll: u16,
    /// Diff ビュー内のカーソル行（0-indexed）
    cursor_line: usize,
    /// Diff ビューの表示可能行数（render 時に更新）
    diff_view_height: u16,
    /// Diff ビューの内部幅（render 時に更新、wrap 計算用）
    diff_view_width: u16,
    /// 行選択モードでの選択状態
    line_selection: Option<LineSelection>,
    /// コメント入力バッファ
    comment_input: String,
    /// 保留中のコメント一覧
    pending_comments: Vec<PendingComment>,
    /// 既存のレビューコメント（GitHub から取得済み）
    review_comments: Vec<ReviewComment>,
    /// 現在表示中のコメント（CommentView モード用）
    viewing_comments: Vec<ReviewComment>,
    /// CommentView ダイアログのスクロール位置
    viewing_comment_scroll: u16,
    /// CommentView ダイアログのスクロール上限（render 時に更新）
    comment_view_max_scroll: u16,
    /// GitHub API クライアント（テスト時は None）
    client: Option<Octocrab>,
    /// ステータスメッセージ（ヘッダーバーに表示、3秒後に自動クリア）
    status_message: Option<StatusMessage>,
    /// レビュー送信フラグ（draw 後に実行するため）
    needs_submit: Option<ReviewEvent>,
    /// レビュー送信ダイアログのカーソル位置（0=Comment, 1=Approve, 2=RequestChanges）
    review_event_cursor: usize,
    /// レビュー本文入力（ReviewBodyInput モード用）
    review_body_input: String,
    /// 送信後に終了するかどうか
    quit_after_submit: bool,
    /// 2キーシーケンスの1文字目（`]` or `[`）を保持
    pending_key: Option<char>,
    /// ヘルプ画面のスクロール位置
    help_scroll: u16,
    /// Zoom モード（フォーカスペインのみ全画面表示）
    zoomed: bool,
    /// Diff ペインの行折り返し（`w` キーでトグル）
    diff_wrap: bool,
    /// Diff ペインの行番号表示（`n` キーでトグル）
    show_line_numbers: bool,
    /// viewed 済みファイル名のセット（コミット跨ぎで維持）
    viewed_files: HashSet<String>,
    /// Diff ハイライトキャッシュ（commit_idx, file_idx, highlighted Text）
    /// ファイル選択が変わらない限り delta を再実行しない
    diff_highlight_cache: Option<(usize, usize, ratatui::text::Text<'static>)>,
    /// Wrap 有効時の視覚行オフセットキャッシュ
    /// offsets[i] = 論理行 i が始まる視覚行番号（render 時に計算）
    diff_visual_offsets: Option<Vec<usize>>,
    /// PR Description のマークダウンレンダリングキャッシュ
    pr_desc_rendered: Option<Text<'static>>,
    /// カラーテーマ（ライト/ダーク）
    theme: ThemeMode,
    /// 各ペインの描画領域（マウスヒットテスト用、render 時に更新）
    pr_desc_rect: Rect,
    commit_list_rect: Rect,
    file_tree_rect: Rect,
    diff_view_rect: Rect,
    /// PR body 中のメディア参照
    media_refs: Vec<MediaRef>,
    /// 画像プロトコル検出結果（None = 画像表示不可）
    picker: Option<Picker>,
    /// ダウンロード済み画像キャッシュ
    media_cache: MediaCache,
    /// メディアビューアの現在のインデックス
    media_viewer_index: usize,
    /// メディアビューアの現在のレンダリング状態（画像のみ、動画は None）
    media_viewer_protocol: Option<StatefulProtocol>,
    /// (commit_sha, filename) → 可視レビューコメント数のキャッシュ（起動時に計算）
    visible_review_comment_cache: HashMap<(String, String), usize>,
    /// 自分のPRかどうか（Approve/Request Changesを非表示にする）
    is_own_pr: bool,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
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
    ) -> Self {
        let mut commit_list_state = ListState::default();
        if !commits.is_empty() {
            commit_list_state.select(Some(0));
        }

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
            commits,
            commit_list_state,
            files_map,
            file_list_state,
            pr_desc_scroll: 0,
            pr_desc_view_height: 10, // 初期値、render で更新される
            pr_desc_visual_total: 0, // 初期値、render で更新される
            diff_scroll: 0,
            cursor_line: 0,
            diff_view_height: 20, // 初期値、render で更新される
            diff_view_width: 80,  // 初期値、render で更新される
            line_selection: None,
            comment_input: String::new(),
            pending_comments: Vec::new(),
            review_comments,
            viewing_comments: Vec::new(),
            viewing_comment_scroll: 0,
            comment_view_max_scroll: 0,
            client,
            status_message: None,
            needs_submit: None,
            review_event_cursor: 0,
            review_body_input: String::new(),
            quit_after_submit: false,
            pending_key: None,
            help_scroll: 0,
            zoomed: false,
            diff_wrap: false,
            show_line_numbers: false,
            viewed_files: HashSet::new(),
            diff_highlight_cache: None,
            diff_visual_offsets: None,
            pr_desc_rendered: None,
            theme,
            pr_desc_rect: Rect::default(),
            commit_list_rect: Rect::default(),
            file_tree_rect: Rect::default(),
            diff_view_rect: Rect::default(),
            media_refs: Vec::new(),
            picker: None,
            media_cache: MediaCache::new(),
            media_viewer_index: 0,
            media_viewer_protocol: None,
            visible_review_comment_cache,
            is_own_pr,
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

    /// メディアビューアモードに入る（メディアがある場合のみ）
    fn enter_media_viewer(&mut self) {
        self.ensure_pr_desc_rendered();
        if self.media_refs.is_empty() {
            self.status_message =
                Some(StatusMessage::info("No images or videos in PR description"));
            return;
        }
        self.media_viewer_index = 0;
        self.prepare_media_protocol();
        self.mode = AppMode::MediaViewer;
    }

    /// 現在の media_viewer_index に対応するメディアのレンダリングプロトコルを準備する。
    /// 動画の場合はプロトコルを作成しない（サムネイル未対応）。
    fn prepare_media_protocol(&mut self) {
        let info = self
            .media_ref_at(self.media_viewer_index)
            .map(|r| (r.media_type.clone(), r.url.clone()));
        let protocol = info.and_then(|(media_type, url)| {
            if media_type == MediaType::Video {
                return None;
            }
            let picker = self.picker.as_ref()?;
            let img = self.media_cache.get(&url)?;
            // new_resize_protocol は DynamicImage を所有で受け取るためクローンが必要
            Some(picker.new_resize_protocol(img.clone()))
        });
        self.media_viewer_protocol = protocol;
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
        self.cursor_line = 0;
        self.diff_scroll = 0;
        // 先頭の @@ 行をスキップ
        let max = self.current_diff_line_count();
        self.cursor_line = self.skip_hunk_header_forward(0, max);
    }

    /// 現在選択中のファイルを取得
    fn current_file(&self) -> Option<&DiffFile> {
        let files = self.current_files();
        if let Some(idx) = self.file_list_state.selected() {
            return files.get(idx);
        }
        None
    }

    /// viewed フラグをトグル（FileTree 用）
    fn toggle_viewed(&mut self) {
        if let Some(file) = self.current_file() {
            let name = file.filename.clone();
            if !self.viewed_files.remove(&name) {
                self.viewed_files.insert(name);
            }
        }
    }

    /// コミットの全ファイルが viewed か判定（導出状態）
    fn is_commit_viewed(&self, sha: &str) -> bool {
        if let Some(files) = self.files_map.get(sha) {
            !files.is_empty()
                && files
                    .iter()
                    .all(|f| self.viewed_files.contains(&f.filename))
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
            for name in &filenames {
                self.viewed_files.remove(name);
            }
        } else {
            // 全ファイルを view
            for name in filenames {
                self.viewed_files.insert(name);
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

        self.review_comments
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

            terminal.draw(|frame| self.render(frame))?;

            // draw 後に submit を実行（ローディング表示を先にユーザーへ見せる）
            if let Some(event) = self.needs_submit.take() {
                self.submit_review_with_event(event);
                if self.quit_after_submit {
                    self.quit_after_submit = false;
                    self.should_quit = true;
                }
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

        // PR タイトルと作者をヘッダー行として先頭に挿入
        let author_part = if self.pr_author.is_empty() {
            String::new()
        } else {
            format!(" by @{}", self.pr_author)
        };
        let title_line = Line::styled(
            format!("{}{}", self.pr_title, author_part),
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
            let options = tui_markdown::Options::new(PrDescStyleSheet { theme: self.theme });
            let rendered = tui_markdown::from_str_with_options(&processed_body, &options);
            // 借用ライフタイムを 'static に変換（各 Span の content を所有文字列化）
            // Line::style（heading/blockquote の色）も保持する
            let mut lines: Vec<Line<'static>> = vec![title_line, separator, Line::raw("")];
            lines.extend(rendered.lines.into_iter().map(|line| {
                let mut new_line = Line::from(
                    line.spans
                        .into_iter()
                        .map(|span| Span::styled(span.content.into_owned(), span.style))
                        .collect::<Vec<_>>(),
                );
                new_line.style = line.style;
                new_line.alignment = line.alignment;
                new_line
            }));

            Text::from(lines)
        };
        self.pr_desc_rendered = Some(text);
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

    /// 座標からペインを特定
    fn panel_at(&self, x: u16, y: u16) -> Option<Panel> {
        let pos = Position::new(x, y);
        if self.pr_desc_rect.contains(pos) {
            Some(Panel::PrDescription)
        } else if self.commit_list_rect.contains(pos) {
            Some(Panel::CommitList)
        } else if self.file_tree_rect.contains(pos) {
            Some(Panel::FileTree)
        } else if self.diff_view_rect.contains(pos) {
            Some(Panel::DiffView)
        } else {
            None
        }
    }

    /// 行選択モードに入る（hunk header 上では無効）
    fn enter_line_select_mode(&mut self) {
        if self.is_hunk_header(self.cursor_line) {
            return;
        }
        // 現在のカーソル行をアンカーとして選択開始
        self.line_selection = Some(LineSelection {
            anchor: self.cursor_line,
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
            self.comment_input.clear();
            self.mode = AppMode::CommentInput;
        }
    }

    /// コメント入力をキャンセルして LineSelect に戻る（選択範囲維持）
    fn cancel_comment_input(&mut self) {
        self.comment_input.clear();
        self.mode = AppMode::LineSelect;
    }

    /// コメントを確定して pending_comments に追加
    fn confirm_comment(&mut self) {
        if self.comment_input.is_empty() {
            return;
        }

        if let Some(selection) = self.line_selection {
            let (start, end) = selection.range(self.cursor_line);
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

            self.pending_comments.push(PendingComment {
                file_path,
                start_line: start,
                end_line: end,
                body: self.comment_input.clone(),
                commit_sha,
            });
        }

        self.comment_input.clear();
        self.line_selection = None;
        self.mode = AppMode::Normal;
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
        if event == ReviewEvent::Comment && self.pending_comments.is_empty() {
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

        let count = self.pending_comments.len();
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
                &self.pending_comments,
                &self.files_map,
                event.as_api_str(),
                &self.review_body_input,
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
                self.pending_comments.clear();
                self.review_body_input.clear();
            }
            Err(e) => {
                self.status_message = Some(StatusMessage::error(format!("✗ Failed: {}", e)));
            }
        }
    }

    /// 選択範囲を下に拡張（カーソルを下に移動）
    fn extend_selection_down(&mut self) {
        let line_count = self.current_diff_line_count();
        let next = self.cursor_line + 1;
        if next < line_count
            && !self.is_hunk_header(next)
            && self.is_same_hunk(self.cursor_line, next)
        {
            self.cursor_line = next;
            self.ensure_cursor_visible();
        }
    }

    /// 選択範囲を上に拡張（カーソルを上に移動）
    fn extend_selection_up(&mut self) {
        if self.cursor_line > 0 {
            let prev = self.cursor_line - 1;
            if !self.is_hunk_header(prev) && self.is_same_hunk(self.cursor_line, prev) {
                self.cursor_line = prev;
                self.ensure_cursor_visible();
            }
        }
    }

    /// @@ hunk header を整形表示用の Line に変換
    /// `@@ -10,5 +12,7 @@ fn main()` → `─── L10-14 → L12-18 ─── fn main() ────`
    fn format_hunk_header(raw: &str, width: u16, style: Style) -> Line<'static> {
        let width = width as usize;

        let (range_text, context) = if let Some(rest) = raw.strip_prefix("@@ ") {
            if let Some(at_pos) = rest.find(" @@") {
                let range_part = &rest[..at_pos];
                let ctx = rest[at_pos + 3..].trim();

                let mut parts = range_part.split_whitespace();
                let old = parts
                    .next()
                    .and_then(|p| p.strip_prefix('-'))
                    .unwrap_or("0");
                let new = parts
                    .next()
                    .and_then(|p| p.strip_prefix('+'))
                    .unwrap_or("0");

                let format_range = |r: &str| -> String {
                    let mut iter = r.split(',');
                    let start: usize = iter.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let len: usize = iter.next().and_then(|s| s.parse().ok()).unwrap_or(1);
                    if len <= 1 {
                        format!("L{start}")
                    } else {
                        format!("L{}-{}", start, start + len - 1)
                    }
                };

                (
                    format!("{} → {}", format_range(old), format_range(new)),
                    ctx.to_string(),
                )
            } else {
                (String::new(), String::new())
            }
        } else {
            (String::new(), String::new())
        };

        let mut content = String::from("─── ");
        if !range_text.is_empty() {
            content.push_str(&range_text);
            content.push(' ');
        }
        if !context.is_empty() {
            content.push_str("─── ");
            content.push_str(&context);
            content.push(' ');
        }

        let content_width = UnicodeWidthStr::width(content.as_str());
        let fill_count = width.saturating_sub(content_width);
        for _ in 0..fill_count {
            content.push('─');
        }

        Line::styled(content, style)
    }
}

/// URL をシステムのデフォルトブラウザで開く
fn open_url_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let _ = std::process::Command::new(cmd).arg(url).spawn();
}

/// 文字列を最大表示幅に収まるように末尾を省略する（unicode-width 対応）
/// 例: "prism - repo#1: Long PR title" → "prism - repo#1: Lo…"
pub(super) fn truncate_str(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut width = 0;
    let mut result = String::new();
    let ellipsis_width = 1; // "…" is 1 column wide
    let target = max_width.saturating_sub(ellipsis_width);
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > target {
            break;
        }
        width += cw;
        result.push(ch);
    }
    result.push('…');
    result
}

/// パスを最大幅に収まるように先頭を省略する（ASCII パスを前提）
/// 例: "src/components/MyComponent/index.tsx" → ".../MyComponent/index.tsx"
pub(super) fn truncate_path(path: &str, max_width: usize) -> String {
    if path.len() <= max_width {
        return path.to_string();
    }
    if max_width < 4 {
        // "..." すら収まらない幅ではそのまま切り詰める
        return path[..max_width].to_string();
    }
    // "..." prefix = 3 chars
    let available = max_width - 3;
    // パスの後ろから available 文字分を取り、最初の '/' 以降を使う
    let tail = &path[path.len() - available..];
    if let Some(pos) = tail.find('/') {
        format!("...{}", &tail[pos..])
    } else {
        format!("...{}", tail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::commits::{CommitDetail, CommitInfo};
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::{Duration, Instant};

    fn create_test_commits() -> Vec<CommitInfo> {
        vec![
            CommitInfo {
                sha: "abc1234567890".to_string(),
                commit: CommitDetail {
                    message: "First commit".to_string(),
                },
            },
            CommitInfo {
                sha: "def4567890123".to_string(),
                commit: CommitDetail {
                    message: "Second commit".to_string(),
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

    fn create_empty_files_map() -> HashMap<String, Vec<DiffFile>> {
        HashMap::new()
    }

    #[test]
    fn test_new_with_empty_commits() {
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        let commits = create_test_commits();
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        assert_eq!(app.commits.len(), 2);
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    #[test]
    fn test_new_with_files() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        assert_eq!(app.files_map.len(), 2);
        assert_eq!(app.file_list_state.selected(), Some(0));
    }

    #[test]
    fn test_next_panel() {
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        let commits = create_test_commits();
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::CommitList;
        assert_eq!(app.commit_list_state.selected(), Some(0));
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1));
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(0)); // wrap around
    }

    #[test]
    fn test_select_prev_commits() {
        let commits = create_test_commits();
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::CommitList;
        assert_eq!(app.commit_list_state.selected(), Some(0));
        app.select_prev();
        assert_eq!(app.commit_list_state.selected(), Some(1)); // wrap around
        app.select_prev();
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    #[test]
    fn test_select_next_files() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::FileTree;
        assert_eq!(app.file_list_state.selected(), Some(0));
        app.select_next();
        assert_eq!(app.file_list_state.selected(), Some(1));
        app.select_next();
        assert_eq!(app.file_list_state.selected(), Some(0)); // wrap around
    }

    #[test]
    fn test_select_prev_files() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::FileTree;
        assert_eq!(app.file_list_state.selected(), Some(0));
        app.select_prev();
        assert_eq!(app.file_list_state.selected(), Some(1)); // wrap around
        app.select_prev();
        assert_eq!(app.file_list_state.selected(), Some(0));
    }

    #[test]
    fn test_select_only_works_in_current_panel() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        let commits = create_test_commits();
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );

        // Verify the commit list state is properly initialized
        assert_eq!(app.commit_list_state.selected(), Some(0));
        assert_eq!(app.commits.len(), 2);
        assert_eq!(app.commits[0].short_sha(), "abc1234");
        assert_eq!(app.commits[0].message_summary(), "First commit");
    }

    #[test]
    fn test_current_files_returns_correct_files() {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "file1.rs".to_string(),
                status: "added".to_string(),
                additions: 10,
                deletions: 0,
                patch: None,
            }],
        );
        files_map.insert(
            "def4567890123".to_string(),
            vec![DiffFile {
                filename: "file2.rs".to_string(),
                status: "modified".to_string(),
                additions: 5,
                deletions: 3,
                patch: None,
            }],
        );

        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );

        // 最初のコミットのファイルが返される
        let files = app.current_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "file1.rs");
    }

    #[test]
    fn test_commit_change_resets_file_selection() {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        files_map.insert(
            "abc1234567890".to_string(),
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
            "def4567890123".to_string(),
            vec![DiffFile {
                filename: "file3.rs".to_string(),
                status: "modified".to_string(),
                additions: 5,
                deletions: 3,
                patch: None,
            }],
        );

        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );

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
        let commits = create_test_commits();
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        assert_eq!(app.diff_scroll, 0);
    }

    #[test]
    fn test_scroll_diff_down() {
        // 10行パッチ、half page = 5
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff_view_height = 10;
        assert_eq!(app.cursor_line, 0);

        app.scroll_diff_down();
        assert_eq!(app.cursor_line, 5); // 半ページ分

        app.scroll_diff_down();
        assert_eq!(app.cursor_line, 9); // 末尾でクランプ (10行-1)
    }

    #[test]
    fn test_scroll_diff_up() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff_view_height = 10;
        app.cursor_line = 9;

        app.scroll_diff_up();
        assert_eq!(app.cursor_line, 4); // 半ページ分戻る

        app.scroll_diff_up();
        assert_eq!(app.cursor_line, 0);

        // 0 以下にはならない
        app.scroll_diff_up();
        assert_eq!(app.cursor_line, 0);
    }

    #[test]
    fn test_scroll_only_works_in_diff_panel() {
        let mut app = create_app_with_patch();
        app.diff_view_height = 10;

        // PrDescription panel (default)
        app.scroll_diff_down();
        assert_eq!(app.cursor_line, 0);

        app.focused_panel = Panel::CommitList;
        app.scroll_diff_down();
        assert_eq!(app.cursor_line, 0);

        app.focused_panel = Panel::FileTree;
        app.scroll_diff_down();
        assert_eq!(app.cursor_line, 0);

        app.focused_panel = Panel::DiffView;
        app.scroll_diff_down();
        assert_eq!(app.cursor_line, 5); // 半ページ分
    }

    #[test]
    fn test_scroll_diff_to_end() {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        // 25行のパッチ
        let patch = (0..25)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "file1.rs".to_string(),
                status: "added".to_string(),
                additions: 25,
                deletions: 0,
                patch: Some(patch),
            }],
        );
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::DiffView;

        app.scroll_diff_to_end();
        assert_eq!(app.cursor_line, 24); // 末尾行 (25-1)
    }

    #[test]
    fn test_file_change_resets_scroll() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.diff_scroll = 50;

        // Change to FileTree and select next file
        app.focused_panel = Panel::FileTree;
        app.select_next();

        // Scroll should be reset
        assert_eq!(app.diff_scroll, 0);
    }

    /// コメント入力テスト用: patch 付きファイルを含む App を作成
    fn create_app_with_patch() -> App {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        let patch = (0..10)
            .map(|i| format!("+line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "added".to_string(),
                additions: 10,
                deletions: 0,
                patch: Some(patch),
            }],
        );
        App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        )
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
        assert!(app.comment_input.is_empty());
    }

    #[test]
    fn test_comment_input_mode_cancel_returns_to_line_select() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;

        // 行選択 → コメント入力
        app.enter_line_select_mode();
        let selection_before = app.line_selection;
        app.enter_comment_input_mode();
        assert_eq!(app.mode, AppMode::CommentInput);

        // Esc で LineSelect に戻る（選択範囲維持）
        app.cancel_comment_input();
        assert_eq!(app.mode, AppMode::LineSelect);
        assert_eq!(app.line_selection, selection_before);
    }

    #[test]
    fn test_comment_input_char_and_backspace() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.enter_line_select_mode();
        app.enter_comment_input_mode();

        // 文字入力
        app.handle_comment_input_mode(KeyCode::Char('H'));
        app.handle_comment_input_mode(KeyCode::Char('i'));
        assert_eq!(app.comment_input, "Hi");

        // Backspace
        app.handle_comment_input_mode(KeyCode::Backspace);
        assert_eq!(app.comment_input, "H");

        // 全文字削除
        app.handle_comment_input_mode(KeyCode::Backspace);
        assert!(app.comment_input.is_empty());

        // 空の状態でさらに Backspace しても panic しない
        app.handle_comment_input_mode(KeyCode::Backspace);
        assert!(app.comment_input.is_empty());
    }

    #[test]
    fn test_comment_confirm_adds_pending_comment() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.enter_line_select_mode();
        app.enter_comment_input_mode();

        // コメント入力
        app.handle_comment_input_mode(KeyCode::Char('L'));
        app.handle_comment_input_mode(KeyCode::Char('G'));
        app.handle_comment_input_mode(KeyCode::Char('T'));
        app.handle_comment_input_mode(KeyCode::Char('M'));

        // Enter で確定
        app.confirm_comment();
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.pending_comments.len(), 1);
        assert_eq!(app.pending_comments[0].body, "LGTM");
        assert_eq!(app.pending_comments[0].file_path, "src/main.rs");
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
        assert!(app.pending_comments.is_empty());
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
    fn test_parse_repo_valid() {
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        let (owner, repo) = app.parse_repo().unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_repo_invalid() {
        let app = App::new(
            1,
            "invalid".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        assert!(app.parse_repo().is_none());
    }

    #[test]
    fn test_submit_with_empty_pending_comments_does_nothing() {
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        assert_eq!(app.review_event_cursor, 0);
    }

    #[test]
    fn test_review_submit_dialog_navigation() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;
        app.review_event_cursor = 0;

        // j で下に移動
        app.handle_review_submit_mode(KeyCode::Char('j'));
        assert_eq!(app.review_event_cursor, 1);
        app.handle_review_submit_mode(KeyCode::Char('j'));
        assert_eq!(app.review_event_cursor, 2);
        // 循環
        app.handle_review_submit_mode(KeyCode::Char('j'));
        assert_eq!(app.review_event_cursor, 0);

        // k で上に移動（循環）
        app.handle_review_submit_mode(KeyCode::Char('k'));
        assert_eq!(app.review_event_cursor, 2);
    }

    #[test]
    fn test_review_submit_comment_requires_pending() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;
        app.review_event_cursor = 0; // Comment

        // pending_comments が空で Comment を選択するとエラー
        app.handle_review_submit_mode(KeyCode::Enter);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.needs_submit.is_none());
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
        app.review_event_cursor = 1; // Approve

        // pending_comments が空でも Approve → ReviewBodyInput に遷移
        app.handle_review_submit_mode(KeyCode::Enter);
        assert_eq!(app.mode, AppMode::ReviewBodyInput);
        assert!(app.review_body_input.is_empty());
        assert!(app.needs_submit.is_none());
    }

    #[test]
    fn test_review_submit_escape_cancels() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;

        app.handle_review_submit_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.needs_submit.is_none());
        assert!(!app.quit_after_submit);
    }

    #[test]
    fn test_review_submit_escape_resets_quit_after_submit() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewSubmit;
        app.quit_after_submit = true; // QuitConfirm → y → ReviewSubmit の流れ

        app.handle_review_submit_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(!app.quit_after_submit);
    }

    #[test]
    fn test_number_keys_jump_to_panels() {
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.handle_normal_mode(KeyCode::Char('2'), KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::CommitList);
        app.handle_normal_mode(KeyCode::Char('3'), KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.handle_normal_mode(KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::PrDescription);
    }

    #[test]
    fn test_enter_in_files_moves_to_diff() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::FileTree;
        app.handle_normal_mode(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::DiffView);
    }

    #[test]
    fn test_esc_in_diff_returns_to_files() {
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::DiffView;
        app.handle_normal_mode(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.focused_panel, Panel::FileTree);
    }

    #[test]
    fn test_tab_skips_diffview() {
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        app.pending_comments.push(PendingComment {
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
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "image.png".to_string(),
                status: "added".to_string(),
                additions: 0,
                deletions: 0,
                patch: None,
            }],
        );
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        assert_eq!(app.current_diff_line_count(), 0);
    }

    #[test]
    fn test_commit_message_summary_vs_full() {
        // message_summary は1行目のみ、commit.message は全文
        let commit = CommitInfo {
            sha: "abc1234567890".to_string(),
            commit: CommitDetail {
                message: "First line\n\nDetailed description\nMore details".to_string(),
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
        app.cursor_line = 3;

        // Normal モードで c キー
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::empty());
        assert_eq!(app.mode, AppMode::CommentInput);
        assert!(app.line_selection.is_some());

        // line_selection のアンカーがカーソル行に設定されている
        let sel = app.line_selection.unwrap();
        assert_eq!(sel.anchor, 3);
        // 単一行なので range は (3, 3)
        assert_eq!(sel.range(app.cursor_line), (3, 3));
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
        app.pending_comments.push(PendingComment {
            file_path: "src/main.rs".to_string(),
            start_line: 2,
            end_line: 4,
            body: "Review this".to_string(),
            commit_sha: "abc1234567890".to_string(),
        });

        // 該当ファイルにペンディングコメントがある
        assert!(
            app.pending_comments
                .iter()
                .any(|c| c.file_path == "src/main.rs")
        );
        // 別のファイルにはない
        assert!(
            !app.pending_comments
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
        app.pending_comments.push(PendingComment {
            file_path: "src/main.rs".to_string(),
            start_line: 0,
            end_line: 0,
            body: "test".to_string(),
            commit_sha: "abc1234567890".to_string(),
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
        app.pending_comments.push(PendingComment {
            file_path: "test.rs".to_string(),
            start_line: 0,
            end_line: 0,
            body: "test".to_string(),
            commit_sha: "abc".to_string(),
        });

        // y → ReviewSubmit ダイアログに遷移（quit_after_submit フラグ付き）
        app.handle_quit_confirm_mode(KeyCode::Char('y'));
        assert_eq!(app.mode, AppMode::ReviewSubmit);
        assert!(app.quit_after_submit);
        assert_eq!(app.review_event_cursor, 0);
    }

    #[test]
    fn test_quit_confirm_n_discards_and_quits() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::QuitConfirm;
        app.pending_comments.push(PendingComment {
            file_path: "test.rs".to_string(),
            start_line: 0,
            end_line: 0,
            body: "test".to_string(),
            commit_sha: "abc".to_string(),
        });

        app.handle_quit_confirm_mode(KeyCode::Char('n'));
        assert!(app.should_quit);
        assert!(app.pending_comments.is_empty());
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
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
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
        assert_eq!(app.cursor_line, 1);

        // Up で選択縮小
        app.handle_line_select_mode(KeyCode::Up);
        assert_eq!(app.cursor_line, 0);
    }

    #[test]
    fn test_panel_at_returns_correct_panel() {
        let mut app = create_app_with_patch();
        // Rect を手動設定（render を経由しないテスト用）
        app.pr_desc_rect = Rect::new(0, 1, 30, 10);
        app.commit_list_rect = Rect::new(0, 11, 30, 10);
        app.file_tree_rect = Rect::new(0, 21, 30, 10);
        app.diff_view_rect = Rect::new(30, 1, 50, 30);

        assert_eq!(app.panel_at(5, 5), Some(Panel::PrDescription));
        assert_eq!(app.panel_at(5, 15), Some(Panel::CommitList));
        assert_eq!(app.panel_at(5, 25), Some(Panel::FileTree));
        assert_eq!(app.panel_at(40, 10), Some(Panel::DiffView));
        assert_eq!(app.panel_at(90, 90), None);
    }

    #[test]
    fn test_mouse_click_changes_focus() {
        let mut app = create_app_with_patch();
        app.pr_desc_rect = Rect::new(0, 1, 30, 10);
        app.commit_list_rect = Rect::new(0, 11, 30, 10);
        app.file_tree_rect = Rect::new(0, 21, 30, 10);
        app.diff_view_rect = Rect::new(30, 1, 50, 30);

        assert_eq!(app.focused_panel, Panel::PrDescription);

        app.handle_mouse_click(40, 10);
        assert_eq!(app.focused_panel, Panel::DiffView);

        app.handle_mouse_click(5, 15);
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_mouse_click_selects_list_item() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        // CommitList: y=11 はボーダー、y=12 が最初のアイテム
        app.commit_list_rect = Rect::new(0, 11, 30, 10);

        // 2番目のアイテム（y=13, offset 0, relative_y=1 → idx=1）をクリック
        app.handle_mouse_click(5, 13);
        assert_eq!(app.focused_panel, Panel::CommitList);
        assert_eq!(app.commit_list_state.selected(), Some(1));
    }

    #[test]
    fn test_mouse_scroll_on_diff() {
        // 10行パッチ、表示5行 → max_scroll = 5
        let mut app = create_app_with_patch();
        app.diff_view_rect = Rect::new(30, 1, 50, 30);
        app.diff_view_height = 5;
        app.focused_panel = Panel::FileTree; // フォーカスは別のペイン

        // 下スクロール → ビューポート+カーソル同時移動（見た目位置固定）
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.diff_scroll, 0);
        app.handle_mouse_scroll(40, 10, true);
        assert_eq!(app.cursor_line, 1);
        assert_eq!(app.diff_scroll, 1);

        // 上スクロール → 元に戻る
        app.handle_mouse_scroll(40, 10, false);
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.diff_scroll, 0);

        // ページ先頭で上スクロール → カーソルのみ（既に0なので動かない）
        app.handle_mouse_scroll(40, 10, false);
        assert_eq!(app.cursor_line, 0);
        assert_eq!(app.diff_scroll, 0);

        // ページ末尾まで下スクロール（max_scroll=5）
        for _ in 0..5 {
            app.handle_mouse_scroll(40, 10, true);
        }
        assert_eq!(app.diff_scroll, 5);
        assert_eq!(app.cursor_line, 5);

        // ページ末尾到達後 → カーソルのみ移動
        app.handle_mouse_scroll(40, 10, true);
        assert_eq!(app.diff_scroll, 5); // ページは動かない
        assert_eq!(app.cursor_line, 6); // カーソルだけ進む

        assert_eq!(app.focused_panel, Panel::FileTree); // フォーカスは変わらない
    }

    #[test]
    fn test_mouse_scroll_on_pr_description() {
        // マークダウンではパラグラフ間に空行が必要（連続行は1段落として結合される）
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            "line1\n\nline2\n\nline3\n\nline4\n\nline5".to_string(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.pr_desc_rect = Rect::new(0, 1, 30, 5);
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
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.commit_list_rect = Rect::new(0, 11, 30, 10);

        // CommitList 上でスクロールしても選択は変わらない
        app.handle_mouse_scroll(5, 15, true);
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    // === N6: viewed フラグテスト ===

    #[test]
    fn test_toggle_viewed() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::FileTree;
        assert!(app.viewed_files.is_empty());

        // トグル → viewed に追加
        app.toggle_viewed();
        assert!(app.viewed_files.contains("src/main.rs"));

        // 再トグル → viewed から削除
        app.toggle_viewed();
        assert!(!app.viewed_files.contains("src/main.rs"));
    }

    #[test]
    fn test_viewed_persists_across_commits() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::FileTree;

        // ファイルを viewed にする
        app.toggle_viewed();
        assert!(app.viewed_files.contains("src/main.rs"));

        // コミットを切り替え
        app.focused_panel = Panel::CommitList;
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1));

        // viewed は維持される
        assert!(app.viewed_files.contains("src/main.rs"));
    }

    #[test]
    fn test_toggle_viewed_no_file_selected() {
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );

        // ファイル未選択時は何もしない（パニックしない）
        app.toggle_viewed();
        assert!(app.viewed_files.is_empty());
    }

    #[test]
    fn test_x_key_toggles_viewed_in_file_tree() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.focused_panel = Panel::FileTree;

        // x キーで viewed トグル
        app.handle_normal_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.viewed_files.contains("src/main.rs"));

        // CommitList では x キーでコミットの全ファイルをトグル
        app.focused_panel = Panel::CommitList;
        app.handle_normal_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        // コミット0 の全ファイル (src/main.rs, src/app.rs) が viewed に
        assert_eq!(app.viewed_files.len(), 2);
        assert!(app.viewed_files.contains("src/main.rs"));
        assert!(app.viewed_files.contains("src/app.rs"));

        // もう一度 x → 全ファイルが unview（既に全て viewed なので）
        app.handle_normal_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.viewed_files.is_empty());
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
            commit_id: "abc1234567890".to_string(),
            user: crate::github::comments::ReviewCommentUser {
                login: "testuser".to_string(),
            },
            created_at: "2025-01-01T00:00:00Z".to_string(),
            in_reply_to_id: None,
        }
    }

    fn create_app_with_comments() -> App {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        // @@ -0,0 +1,3 @$ +line1 +line2 +line3
        let patch = "@@ -0,0 +1,3 @@\n+line1\n+line2\n+line3".to_string();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "added".to_string(),
                additions: 3,
                deletions: 0,
                patch: Some(patch),
            }],
        );
        let comments = vec![make_review_comment(
            "src/main.rs",
            Some(2),
            "RIGHT",
            "Nice line!",
        )];
        App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            comments,
            None,
            ThemeMode::Dark,
            false,
        )
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
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "added".to_string(),
                additions: 1,
                deletions: 0,
                patch: Some("@@ -0,0 +1 @@\n+line1".to_string()),
            }],
        );
        // outdated コメント (line=None) はスキップされる
        let comments = vec![make_review_comment(
            "src/main.rs",
            None,
            "RIGHT",
            "Outdated comment",
        )];
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            comments,
            None,
            ThemeMode::Dark,
            false,
        );
        let counts = app.existing_comment_counts();
        assert!(counts.is_empty());
    }

    #[test]
    fn test_existing_comment_counts_no_match() {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "added".to_string(),
                additions: 1,
                deletions: 0,
                patch: Some("@@ -0,0 +1 @@\n+line1".to_string()),
            }],
        );
        // 別ファイルのコメントはマッチしない
        let comments = vec![make_review_comment(
            "other.rs",
            Some(1),
            "RIGHT",
            "Wrong file",
        )];
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            comments,
            None,
            ThemeMode::Dark,
            false,
        );
        let counts = app.existing_comment_counts();
        assert!(counts.is_empty());
    }

    #[test]
    fn test_enter_opens_comment_view_on_comment_line() {
        let mut app = create_app_with_comments();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 2; // +line2 (コメントがある行)

        app.handle_normal_mode(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::CommentView);
        assert_eq!(app.viewing_comments.len(), 1);
        assert_eq!(app.viewing_comments[0].body, "Nice line!");
    }

    #[test]
    fn test_enter_does_not_open_comment_view_on_empty_line() {
        let mut app = create_app_with_comments();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 1; // +line1 (コメントがない行)

        app.handle_normal_mode(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.viewing_comments.is_empty());
    }

    #[test]
    fn test_comment_view_esc_closes() {
        let mut app = create_app_with_comments();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 2;

        // CommentView を開く
        app.handle_normal_mode(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.mode, AppMode::CommentView);

        // Esc で閉じる
        app.handle_comment_view_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.viewing_comments.is_empty());
    }

    /// 複数 hunk のパッチを持つ App を作成するヘルパー
    fn create_app_with_multi_hunk_patch() -> App {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        // hunk1: 行0-3, hunk2: 行4-7
        let patch = "@@ -1,3 +1,3 @@\n context\n-old line\n+new line\n@@ -10,3 +10,3 @@\n context2\n-old2\n+new2"
            .to_string();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "modified".to_string(),
                additions: 2,
                deletions: 2,
                patch: Some(patch),
            }],
        );
        App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        )
    }

    #[test]
    fn test_hunk_boundary_blocks_selection_down() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // カーソルを hunk1 の最後の行 (行3: "+new line") に移動
        app.cursor_line = 3;
        app.enter_line_select_mode();

        // 行4 は @@ (hunk2 ヘッダー) → 別 hunk なので移動不可
        app.extend_selection_down();
        assert_eq!(app.cursor_line, 3); // 移動しない
    }

    #[test]
    fn test_hunk_boundary_blocks_selection_up() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // カーソルを hunk2 の最初のコンテンツ行 (行5) に配置
        app.cursor_line = 5;
        app.enter_line_select_mode();

        // 行4 は @@ ヘッダー → カーソル不可なので移動しない
        app.extend_selection_up();
        assert_eq!(app.cursor_line, 5); // @@ 行にはカーソルを置けない
    }

    #[test]
    fn test_selection_within_same_hunk() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // hunk1 内 (行0) から選択開始
        app.cursor_line = 0;
        app.enter_line_select_mode();

        // hunk1 内で自由に移動できる
        app.extend_selection_down(); // 行1
        assert_eq!(app.cursor_line, 1);
        app.extend_selection_down(); // 行2
        assert_eq!(app.cursor_line, 2);
        app.extend_selection_down(); // 行3
        assert_eq!(app.cursor_line, 3);
        // 行4 (@@) は別 hunk → 停止
        app.extend_selection_down();
        assert_eq!(app.cursor_line, 3);
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
        app.cursor_line = 0;
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
        app.cursor_line = 4;
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);
        // @@ 行上ではコメント入力に入れない
        assert_eq!(app.mode, AppMode::Normal);
        assert!(app.line_selection.is_none());
    }

    #[test]
    fn test_page_down_moves_cursor_by_view_height() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff_view_height = 3;
        app.cursor_line = 0;

        app.page_down();
        assert_eq!(app.cursor_line, 3);

        app.page_down();
        assert_eq!(app.cursor_line, 6);
    }

    #[test]
    fn test_page_up_moves_cursor_by_view_height() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff_view_height = 3;
        app.cursor_line = 7;

        app.page_up();
        assert_eq!(app.cursor_line, 4);

        app.page_up();
        assert_eq!(app.cursor_line, 1);

        app.page_up();
        assert_eq!(app.cursor_line, 0); // 0 で停止
    }

    #[test]
    fn test_ctrl_f_b_keybinds() {
        let mut app = create_app_with_patch();
        app.focused_panel = Panel::DiffView;
        app.diff_view_height = 3;

        app.handle_normal_mode(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(app.cursor_line, 3);

        app.handle_normal_mode(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(app.cursor_line, 0);
    }

    #[test]
    fn test_jump_to_next_change() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        // 行0: @@, 行1: context, 行2: -old, 行3: +new, 行4: @@, 行5: context2, 行6: -old2, 行7: +new2
        app.cursor_line = 0;

        app.jump_to_next_change();
        assert_eq!(app.cursor_line, 2); // ブロックA先頭 (-old line)

        app.jump_to_next_change();
        assert_eq!(app.cursor_line, 6); // ブロックB先頭 (-old2)、ブロックA全体をスキップ

        // それ以降にブロックがないのでカーソルは動かない
        app.jump_to_next_change();
        assert_eq!(app.cursor_line, 6);
    }

    #[test]
    fn test_jump_to_prev_change() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 7; // +new2 (ブロックB末尾)

        app.jump_to_prev_change();
        assert_eq!(app.cursor_line, 6); // ブロックB先頭 (-old2)

        app.jump_to_prev_change();
        assert_eq!(app.cursor_line, 2); // ブロックA先頭 (-old line)

        // それ以前にブロックがないのでカーソルは動かない
        app.jump_to_prev_change();
        assert_eq!(app.cursor_line, 2);
    }

    #[test]
    fn test_jump_to_next_hunk() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 1; // 最初の hunk 内

        app.jump_to_next_hunk();
        assert_eq!(app.cursor_line, 5); // 2番目の @@ の次の実コード行

        // それ以降に @@ がないのでカーソルは動かない
        app.jump_to_next_hunk();
        assert_eq!(app.cursor_line, 5);
    }

    #[test]
    fn test_jump_to_prev_hunk() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 7; // 最終行

        app.jump_to_prev_hunk();
        assert_eq!(app.cursor_line, 5); // 2番目の @@ の次の実コード行

        app.jump_to_prev_hunk();
        assert_eq!(app.cursor_line, 1); // 最初の @@ の次の実コード行
    }

    #[test]
    fn test_two_key_sequence_bracket_c() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 0;

        // ]c → 次の変更行
        app.handle_normal_mode(KeyCode::Char(']'), KeyModifiers::NONE);
        assert!(app.pending_key.is_some());
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(app.pending_key.is_none());
        assert_eq!(app.cursor_line, 2); // -old line

        // [c → 前の変更行
        app.cursor_line = 7;
        app.handle_normal_mode(KeyCode::Char('['), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(app.cursor_line, 6); // -old2
    }

    #[test]
    fn test_two_key_sequence_bracket_h() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 1;

        // ]h → 次の hunk の実コード行
        app.handle_normal_mode(KeyCode::Char(']'), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.cursor_line, 5);

        // [h → 前の hunk の実コード行
        app.handle_normal_mode(KeyCode::Char('['), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.cursor_line, 1);
    }

    #[test]
    fn test_two_key_sequence_invalid_second_key() {
        let mut app = create_app_with_multi_hunk_patch();
        app.focused_panel = Panel::DiffView;
        app.cursor_line = 0;

        // ]x → 不明な2文字目は無視、pending_key はクリアされる
        app.handle_normal_mode(KeyCode::Char(']'), KeyModifiers::NONE);
        app.handle_normal_mode(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.pending_key.is_none());
        assert_eq!(app.cursor_line, 0); // 動かない
    }

    // === N12: Zoom モードテスト ===

    #[test]
    fn test_zoom_toggle() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );

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
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );

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
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );

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
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.diff_wrap = true;
        // line 0 → row 0, line 1 → row 1, line 2 → row 3, line 3 → row 4, total → 7
        app.diff_visual_offsets = Some(vec![0, 1, 3, 4, 7]);

        assert_eq!(app.visual_line_offset(0), 0);
        assert_eq!(app.visual_line_offset(1), 1);
        assert_eq!(app.visual_line_offset(2), 3);
        assert_eq!(app.visual_line_offset(3), 4);
        assert_eq!(app.visual_line_offset(4), 7); // 合計表示行数
    }

    // キャッシュから表示行→論理行の逆引きが正しく行われることを検証
    #[test]
    fn test_visual_to_logical_line_with_cache() {
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.diff_wrap = true;
        // line 0 → row 0, line 1 → rows 1-2, line 2 → row 3, line 3 → rows 4-6, total → 7
        app.diff_visual_offsets = Some(vec![0, 1, 3, 4, 7]);

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
        let app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            vec![],
            create_empty_files_map(),
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        // diff_wrap はデフォルトで false

        assert_eq!(app.visual_line_offset(0), 0);
        assert_eq!(app.visual_line_offset(5), 5);
        assert_eq!(app.visual_to_logical_line(5), 5);
    }

    /// 長い行を含むパッチで wrap + 行番号の visual_line_offset を検証
    #[test]
    fn test_visual_line_offset_with_line_numbers() {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        let long_line = format!("+{}", "x".repeat(120));
        let patch = format!("@@ -1,3 +1,3 @@\n context\n-old\n{}", long_line);
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "modified".to_string(),
                additions: 1,
                deletions: 1,
                patch: Some(patch),
            }],
        );
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.diff_view_width = 80;
        app.diff_wrap = true;
        app.show_line_numbers = true;

        let with_numbers = app.visual_line_offset(4);
        assert!(
            with_numbers > 4,
            "行番号ONで長い行は wrap により視覚行数が論理行数より多い"
        );

        app.show_line_numbers = false;
        let without_numbers = app.visual_line_offset(4);
        assert!(
            with_numbers >= without_numbers,
            "行番号ONは行番号OFFより視覚行数が多い（もしくは同じ）"
        );
    }

    /// wrap + 行番号で ensure_cursor_visible がカーソルを画面内に収める
    #[test]
    fn test_ensure_cursor_visible_with_wrap_and_line_numbers() {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        let lines: Vec<String> = (0..20)
            .map(|i| format!("+{}", format!("line{} ", i).repeat(20)))
            .collect();
        let patch = format!("@@ -0,0 +1,20 @@\n{}", lines.join("\n"));
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "added".to_string(),
                additions: 20,
                deletions: 0,
                patch: Some(patch),
            }],
        );
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.diff_view_width = 80;
        app.diff_view_height = 10;
        app.diff_wrap = true;
        app.show_line_numbers = true;
        app.focused_panel = Panel::DiffView;

        app.cursor_line = 20;
        app.ensure_cursor_visible();

        let cursor_visual = app.visual_line_offset(app.cursor_line);
        let cursor_visual_end = app.visual_line_offset(app.cursor_line + 1);
        let scroll = app.diff_scroll as usize;
        let visible = app.diff_view_height as usize;

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
        let commits = create_test_commits();

        // modified ファイル → 両カラム 11文字
        let mut files_map = HashMap::new();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "modified".to_string(),
                additions: 1,
                deletions: 1,
                patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
            }],
        );
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits.clone(),
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.show_line_numbers = true;
        assert_eq!(app.line_number_prefix_width(), 11);

        // added ファイル → 片カラム 6文字
        let mut files_map = HashMap::new();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/new.rs".to_string(),
                status: "added".to_string(),
                additions: 1,
                deletions: 0,
                patch: Some("@@ -0,0 +1 @@\n+new".to_string()),
            }],
        );
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            false,
        );
        app.show_line_numbers = true;
        assert_eq!(app.line_number_prefix_width(), 6);

        // 行番号OFF → 0文字
        app.show_line_numbers = false;
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
        app.review_event_cursor = 1; // Approve

        // 文字入力
        app.handle_review_body_input_mode(KeyCode::Char('L'));
        app.handle_review_body_input_mode(KeyCode::Char('G'));
        app.handle_review_body_input_mode(KeyCode::Char('T'));
        app.handle_review_body_input_mode(KeyCode::Char('M'));
        assert_eq!(app.review_body_input, "LGTM");

        // Backspace
        app.handle_review_body_input_mode(KeyCode::Backspace);
        assert_eq!(app.review_body_input, "LGT");
    }

    #[test]
    fn test_review_body_input_enter_submits() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        app.review_event_cursor = 1; // Approve
        app.review_body_input = "LGTM!".to_string();

        app.handle_review_body_input_mode(KeyCode::Enter);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.needs_submit, Some(ReviewEvent::Approve));
        assert!(app.status_message.is_some());
    }

    #[test]
    fn test_review_body_input_empty_body_submits() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        app.review_event_cursor = 1; // Approve

        // 空bodyでも送信可能
        app.handle_review_body_input_mode(KeyCode::Enter);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.needs_submit, Some(ReviewEvent::Approve));
    }

    #[test]
    fn test_review_body_input_esc_returns_to_submit() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        app.review_body_input = "some text".to_string();

        app.handle_review_body_input_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::ReviewSubmit);
        assert!(app.review_body_input.is_empty());
        assert!(app.needs_submit.is_none());
    }

    #[test]
    fn test_review_body_input_esc_preserves_quit_after_submit() {
        let mut app = create_app_with_patch();
        app.mode = AppMode::ReviewBodyInput;
        app.quit_after_submit = true;

        // Esc で ReviewSubmit に戻る（quit_after_submit はリセットしない）
        app.handle_review_body_input_mode(KeyCode::Esc);
        assert_eq!(app.mode, AppMode::ReviewSubmit);
        assert!(app.quit_after_submit);
    }

    // --- is_own_pr テスト ---

    fn create_own_pr_app() -> App {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        files_map.insert(
            "abc1234567890".to_string(),
            vec![DiffFile {
                filename: "src/main.rs".to_string(),
                status: "added".to_string(),
                additions: 1,
                deletions: 0,
                patch: Some("+line1".to_string()),
            }],
        );
        App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            String::new(),
            String::new(),
            commits,
            files_map,
            vec![],
            None,
            ThemeMode::Dark,
            true,
        )
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
        assert_eq!(app.review_event_cursor, 0);
        app.handle_review_submit_mode(KeyCode::Char('k'));
        assert_eq!(app.review_event_cursor, 0);
        app.handle_review_submit_mode(KeyCode::Down);
        assert_eq!(app.review_event_cursor, 0);
        app.handle_review_submit_mode(KeyCode::Up);
        assert_eq!(app.review_event_cursor, 0);
    }
}
