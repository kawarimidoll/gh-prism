use super::*;

use crate::git::diff::highlight_diff;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, HorizontalAlignment, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};
use ratatui_image::StatefulImage;
use unicode_width::UnicodeWidthStr;

/// コミットメッセージペインの高さ（ボーダー上下 2 + 内容 4 行）
const COMMIT_MSG_HEIGHT: u16 = 6;
/// コメントペインの高さ（ボーダー上下 2 + 内容 4 行）
const COMMENT_PANE_HEIGHT: u16 = 6;

// --- レイアウト比率 ---
const SIDEBAR_WIDTH_PCT: u16 = 30;
const DIFF_WIDTH_PCT: u16 = 70;
const PR_DESC_HEIGHT_PCT: u16 = 40;
const COMMIT_LIST_HEIGHT_PCT: u16 = 30;
const FILE_TREE_HEIGHT_PCT: u16 = 30;

// --- パネルキーヒント ---
const HINT_MEDIA: &str = " o: media ";
const HINT_VIEWED: &str = " x: viewed ";
const HINT_COMMENT: &str = " c: comment ";
const HINT_SELECT_COMMENT: &str = " v: select | c: comment ";

// --- ダイアログサイズ ---
const REVIEW_DIALOG_WIDTH: u16 = 36;
const REVIEW_DIALOG_HEIGHT: u16 = 7;
const QUIT_DIALOG_WIDTH: u16 = 38;
const QUIT_DIALOG_HEIGHT: u16 = 9;
const HELP_DIALOG_WIDTH: u16 = 60;
const HELP_DIALOG_MIN_HEIGHT: u16 = 20;
const HELP_KEY_COLUMN_WIDTH: usize = 20;

// --- 行番号フォーマット ---
const LINE_NUM_WIDTH: usize = 4;
/// LINE_NUM_WIDTH + 1(trailing space) の空白文字列
const LINE_NUM_BLANK: &str = "     ";

// --- テーマカラー ---
const CURSOR_BG_DARK: Color = Color::DarkGray;
const CURSOR_BG_LIGHT: Color = Color::Indexed(254);
const PENDING_BG_DARK: Color = Color::Indexed(22);
const PENDING_BG_LIGHT: Color = Color::Indexed(151);

impl App {
    pub(super) fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        // ReviewBodyInput のみ全幅エディタパネルを下部に表示
        let main_layout = if self.mode == AppMode::ReviewBodyInput {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Min(0),
                    Constraint::Length(COMMENT_PANE_HEIGHT),
                ])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(area)
        };

        let mode_indicator = match self.mode {
            AppMode::Normal => "",
            AppMode::LineSelect => " [LINE SELECT] ",
            AppMode::CommentInput | AppMode::IssueCommentInput => " [COMMENT] ",
            AppMode::CommentView => " [VIEWING] ",
            AppMode::ReviewSubmit => " [REVIEW] ",
            AppMode::ReviewBodyInput => " [REVIEW] ",
            AppMode::QuitConfirm => " [CONFIRM] ",
            AppMode::Help => " [HELP] ",
            AppMode::MediaViewer => " [MEDIA] ",
        };

        let comments_badge = if self.review.pending_comments.is_empty() {
            String::new()
        } else {
            format!(" [{}💬]", self.review.pending_comments.len())
        };

        let header_bg = match self.mode {
            AppMode::Normal => Color::Blue,
            AppMode::LineSelect => Color::Magenta,
            AppMode::CommentInput | AppMode::IssueCommentInput => Color::Green,
            AppMode::CommentView => Color::Yellow,
            AppMode::ReviewSubmit => Color::Cyan,
            AppMode::ReviewBodyInput => Color::Green,
            AppMode::QuitConfirm => Color::Red,
            AppMode::Help => Color::DarkGray,
            AppMode::MediaViewer => Color::DarkGray,
        };
        // CommentView / ReviewSubmit は明るい bg なので常に Black。
        // 他のモードはテーマに応じて White / Black を切り替え。
        let header_fg = match self.mode {
            AppMode::CommentView | AppMode::ReviewSubmit | AppMode::ReviewBodyInput => Color::Black,
            _ => match self.theme {
                ThemeMode::Dark => Color::White,
                ThemeMode::Light => Color::Black,
            },
        };
        let header_style = Style::default().bg(header_bg).fg(header_fg);

        let zoom_indicator = if self.zoomed { " [ZOOM]" } else { "" };

        // 右セクション: モード / ステータス / ズーム / コメントバッジ（固定幅、右端に配置）
        let mut right_spans: Vec<Span> = Vec::new();
        if !mode_indicator.is_empty() {
            right_spans.push(Span::styled(mode_indicator, header_style));
        }
        if !zoom_indicator.is_empty() {
            right_spans.push(Span::styled(zoom_indicator, header_style));
        }
        if !comments_badge.is_empty() {
            right_spans.push(Span::styled(&comments_badge, header_style));
        }
        if let Some(ref msg) = self.status_message {
            let status_style = match msg.level {
                StatusLevel::Info => Style::default().bg(Color::Green).fg(Color::Black),
                StatusLevel::Error => Style::default().bg(Color::Red).fg(Color::White),
            };
            right_spans.push(Span::styled(format!(" {} ", msg.body), status_style));
        }
        let right_width: usize = right_spans.iter().map(|s| s.width()).sum();

        // 左セクション: PR 情報（残り幅で truncate）
        let total_width = main_layout[0].width as usize;
        let left_full = format!(
            " prism - {}#{} | z: zoom | ?: help",
            self.repo, self.pr_number,
        );
        let left_max = total_width.saturating_sub(right_width);
        let left_text = truncate_str(&left_full, left_max);

        let left_used = left_text.width();
        let mut spans = vec![Span::styled(left_text, header_style)];
        // 左と右の間の余白を埋める
        if left_used + right_width < total_width {
            let pad = total_width - left_used - right_width;
            spans.push(Span::styled(" ".repeat(pad), header_style));
        }
        spans.extend(right_spans);

        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(header_style),
            main_layout[0],
        );

        if self.zoomed {
            // Zoom: フォーカスペインのみ全画面表示
            let full_area = main_layout[1];

            // 非表示ペインの Rect をリセット（マウスヒットテスト対策）
            self.layout = LayoutCache::default();

            match self.focused_panel {
                Panel::PrDescription => {
                    self.layout.pr_desc_rect = full_area;
                    self.render_pr_description(frame, full_area);
                }
                Panel::CommitList => {
                    self.layout.commit_list_rect = full_area;
                    self.render_commit_list_stateful(frame, full_area);
                }
                Panel::FileTree => {
                    self.layout.file_tree_rect = full_area;
                    self.render_file_tree(frame, full_area);
                }
                Panel::CommitMessage => {
                    self.layout.commit_msg_rect = full_area;
                    self.render_commit_message(frame, full_area);
                }
                Panel::Conversation => {
                    self.layout.conversation_rect = full_area;
                    self.render_conversation_pane(frame, full_area);
                }
                Panel::DiffView => {
                    if self.mode == AppMode::ReviewBodyInput {
                        // ReviewBodyInput 時は全幅パネルで描画するため CommitMsg + DiffView のみ
                        let zoom_layout = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([
                                Constraint::Length(COMMIT_MSG_HEIGHT),
                                Constraint::Min(0),
                            ])
                            .split(full_area);
                        self.layout.commit_msg_rect = zoom_layout[0];
                        self.layout.diff_view_rect = zoom_layout[1];
                        self.render_commit_message(frame, zoom_layout[0]);
                        self.render_diff_view_widget(frame, zoom_layout[1]);
                    } else {
                        let zoom_layout = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([
                                Constraint::Length(COMMIT_MSG_HEIGHT),
                                Constraint::Min(0),
                                Constraint::Length(COMMENT_PANE_HEIGHT),
                            ])
                            .split(full_area);
                        self.layout.commit_msg_rect = zoom_layout[0];
                        self.layout.diff_view_rect = zoom_layout[1];
                        self.render_commit_message(frame, zoom_layout[0]);
                        self.render_diff_view_widget(frame, zoom_layout[1]);
                        self.render_editor_panel(frame, zoom_layout[2]);
                    }
                }
            }
        } else {
            // 通常表示: サイドバー30% + Diff70%
            let body_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(SIDEBAR_WIDTH_PCT),
                    Constraint::Percentage(DIFF_WIDTH_PCT),
                ])
                .split(main_layout[1]);

            let sidebar_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(PR_DESC_HEIGHT_PCT),
                    Constraint::Percentage(COMMIT_LIST_HEIGHT_PCT),
                    Constraint::Percentage(FILE_TREE_HEIGHT_PCT),
                ])
                .split(body_layout[0]);

            // body_layout[1] を CommitMsg + DiffView + CommentPane に縦分割
            let right_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(COMMIT_MSG_HEIGHT),
                    Constraint::Min(0),
                    Constraint::Length(COMMENT_PANE_HEIGHT),
                ])
                .split(body_layout[1]);

            let commit_msg_area = right_layout[0];
            let diff_area = right_layout[1];
            let comment_area = right_layout[2];

            // マウスヒットテスト用に各ペインの Rect を記録
            self.layout.pr_desc_rect = sidebar_layout[0];
            self.layout.commit_list_rect = sidebar_layout[1];
            self.layout.file_tree_rect = sidebar_layout[2];

            // サイドバー3ペイン描画
            self.render_pr_description(frame, sidebar_layout[0]);
            self.render_commit_list_stateful(frame, sidebar_layout[1]);
            self.render_file_tree(frame, sidebar_layout[2]);

            // 右カラム描画: PrDescription/Conversation フォーカス時は Info+Conversation に切替
            let show_conversation = matches!(
                self.focused_panel,
                Panel::PrDescription | Panel::Conversation
            ) || self.mode == AppMode::IssueCommentInput;

            if show_conversation {
                self.layout.commit_msg_rect = Rect::default();
                self.layout.diff_view_rect = Rect::default();
                self.layout.conversation_rect = diff_area;

                self.render_info_pane(frame, commit_msg_area);
                self.render_conversation_pane(frame, diff_area);
            } else {
                self.layout.commit_msg_rect = commit_msg_area;
                self.layout.diff_view_rect = diff_area;
                self.layout.conversation_rect = Rect::default();

                self.render_commit_message(frame, commit_msg_area);
                self.render_diff_view_widget(frame, diff_area);
            }
            // コメントペイン（ReviewBodyInput 時は全幅パネルで描画するためスキップ）
            if self.mode != AppMode::ReviewBodyInput {
                self.render_editor_panel(frame, comment_area);
            }
        }

        // ReviewBodyInput のみ全幅エディタパネルを描画
        if self.mode == AppMode::ReviewBodyInput {
            self.render_editor_panel(frame, main_layout[2]);
        }

        // ダイアログ描画（画面中央にオーバーレイ）
        match self.mode {
            AppMode::ReviewSubmit => self.render_review_submit_dialog(frame, area),
            AppMode::QuitConfirm => self.render_quit_confirm_dialog(frame, area),
            AppMode::Help => self.render_help_dialog(frame, area),
            AppMode::MediaViewer => self.render_media_viewer_overlay(frame, area),
            _ => {}
        }
    }

    fn render_pr_description(&mut self, frame: &mut Frame, area: Rect) {
        // ボーダー分を引いた表示可能行数を記録
        self.pr_desc_view_height = area.height.saturating_sub(2);
        // ボーダー左右分を引いた内部幅
        let inner_width = area.width.saturating_sub(2);

        let style = if self.focused_panel == Panel::PrDescription {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        self.ensure_pr_desc_rendered();

        // Paragraph::new は Text をムーブするため clone が必要
        let text = self.pr_desc_rendered.as_ref().unwrap().clone();

        // block なしで line_count を計算（block 付きだとボーダー行が加算されてしまう）
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        self.pr_desc_visual_total = paragraph.line_count(inner_width) as u16;
        // zoom 切替等で描画幅が変わった場合にスクロール位置をクランプ
        self.clamp_pr_desc_scroll();

        let mut block = Block::default()
            .title(" PR Description ")
            .borders(Borders::ALL)
            .border_style(style);
        if self.focused_panel == Panel::PrDescription {
            block =
                block.title_bottom(Line::from(HINT_MEDIA).alignment(HorizontalAlignment::Right));
        }
        let paragraph = paragraph.block(block).scroll((self.pr_desc_scroll, 0));

        frame.render_widget(paragraph, area);

        Self::render_scrollbar(
            frame,
            area,
            self.pr_desc_visual_total as usize,
            self.pr_desc_scroll as usize,
            self.pr_desc_view_height as usize,
        );
    }

    fn render_commit_list_stateful(&mut self, frame: &mut Frame, area: Rect) {
        let style = if self.focused_panel == Panel::CommitList {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let items: Vec<ListItem> = self
            .commits
            .iter()
            .map(|c| {
                let viewed = self.is_commit_viewed(&c.sha);
                let marker = if viewed { "✓ " } else { "  " };
                let item_style = if viewed {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                // キャッシュから可視コメント数を取得 + pending を加算
                let comment_count = self
                    .files_map
                    .get(&c.sha)
                    .map(|files| {
                        let mut count = 0usize;
                        for f in files {
                            count += self.cached_visible_comment_count(&c.sha, &f.filename);
                            count += self
                                .review
                                .pending_comments
                                .iter()
                                .filter(|pc| pc.commit_sha == c.sha && pc.file_path == f.filename)
                                .count();
                        }
                        count
                    })
                    .unwrap_or(0);
                let left_part = format!("{}{} {}", marker, c.short_sha(), c.message_summary());
                // ボーダー左右 (2) を除いた内部幅
                let inner = area.width.saturating_sub(2) as usize;
                if comment_count > 0 {
                    let badge = format!("💬 {} ", comment_count);
                    let badge_width = UnicodeWidthStr::width(badge.as_str());
                    let text_max = inner.saturating_sub(badge_width);
                    let left_text = truncate_str(&left_part, text_max);
                    let left_width = UnicodeWidthStr::width(left_text.as_str());
                    let pad = inner.saturating_sub(left_width + badge_width);
                    ListItem::new(Line::from(vec![
                        Span::styled(left_text, item_style),
                        Span::styled(" ".repeat(pad), item_style),
                        Span::styled(badge, Style::default().fg(Color::Yellow)),
                    ]))
                } else {
                    let left_text = truncate_str(&left_part, inner);
                    ListItem::new(Line::from(vec![Span::styled(left_text, item_style)]))
                }
            })
            .collect();

        let viewed_count = self.viewed_commit_count();
        let selected = self
            .commit_list_state
            .selected()
            .map(|i| i + 1)
            .unwrap_or(0);
        let title = format!(
            " Commits {}/{} ✓{} ",
            selected,
            self.commits.len(),
            viewed_count
        );
        let mut block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(style);
        if self.focused_panel == Panel::CommitList {
            block =
                block.title_bottom(Line::from(HINT_VIEWED).alignment(HorizontalAlignment::Right));
        }
        let list = List::new(items)
            .block(block)
            .highlight_style(self.highlight_style());

        let total = self.commits.len();
        frame.render_stateful_widget(list, area, &mut self.commit_list_state);

        let offset = self.commit_list_state.offset();
        let vh = area.height.saturating_sub(2) as usize;
        Self::render_scrollbar(frame, area, total, offset, vh);
    }

    fn render_file_tree(&mut self, frame: &mut Frame, area: Rect) {
        let style = if self.focused_panel == Panel::FileTree {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let files = self.current_files();
        let current_sha = self.current_commit_sha();
        let viewed_count = files
            .iter()
            .filter(|f| {
                current_sha
                    .as_ref()
                    .is_some_and(|sha| self.is_file_viewed(sha, &f.filename))
            })
            .count();
        let items: Vec<ListItem> = files
            .iter()
            .map(|f| {
                let is_viewed = current_sha
                    .as_ref()
                    .is_some_and(|sha| self.is_file_viewed(sha, &f.filename));
                let status = f.status_char();
                let status_color = if is_viewed {
                    Color::DarkGray
                } else {
                    match status {
                        'A' => Color::Green,
                        'M' => Color::Yellow,
                        'D' => Color::Red,
                        'R' => Color::Cyan,
                        _ => Color::White,
                    }
                };
                let text_style = if is_viewed {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let marker = if is_viewed { "✓ " } else { "  " };
                // キャッシュから可視コメント数を取得 + 当該コミットの pending を加算
                let visible_existing = current_sha
                    .as_deref()
                    .map(|sha| self.cached_visible_comment_count(sha, &f.filename))
                    .unwrap_or(0);
                let visible_pending = self
                    .review
                    .pending_comments
                    .iter()
                    .filter(|pc| {
                        pc.file_path == f.filename
                            && current_sha
                                .as_deref()
                                .is_some_and(|sha| sha == pc.commit_sha)
                    })
                    .count();
                let comment_count = visible_existing + visible_pending;
                // ボーダー左右 (2) を除いた内部幅
                let inner = area.width.saturating_sub(2) as usize;
                let status_str = String::from(status);
                let prefix_width = UnicodeWidthStr::width(marker)
                    + UnicodeWidthStr::width(status_str.as_str())
                    + 1; // space before filename
                let (badge, badge_width) = if comment_count > 0 {
                    let b = format!("💬 {} ", comment_count);
                    let w = UnicodeWidthStr::width(b.as_str());
                    (Some(b), w)
                } else {
                    (None, 0)
                };
                let filename_max = inner.saturating_sub(prefix_width + badge_width);
                let truncated = truncate_str(&f.filename, filename_max);
                let mut spans = vec![
                    Span::styled(marker, text_style),
                    Span::styled(status_str, Style::default().fg(status_color)),
                    Span::styled(format!(" {}", truncated), text_style),
                ];
                if let Some(badge) = badge {
                    let left_width = prefix_width + UnicodeWidthStr::width(truncated.as_str());
                    let pad = inner.saturating_sub(left_width + badge_width);
                    spans.push(Span::styled(" ".repeat(pad), text_style));
                    spans.push(Span::styled(badge, Style::default().fg(Color::Yellow)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let selected = self.file_list_state.selected().map(|i| i + 1).unwrap_or(0);
        let total = items.len();
        let title = format!(" Files {}/{} ✓{} ", selected, files.len(), viewed_count);
        let mut block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(style);
        if self.focused_panel == Panel::FileTree {
            block =
                block.title_bottom(Line::from(HINT_VIEWED).alignment(HorizontalAlignment::Right));
        }
        let list = List::new(items)
            .block(block)
            .highlight_style(self.highlight_style());

        frame.render_stateful_widget(list, area, &mut self.file_list_state);

        let offset = self.file_list_state.offset();
        let vh = area.height.saturating_sub(2) as usize;
        Self::render_scrollbar(frame, area, total, offset, vh);
    }

    fn render_commit_message(&mut self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focused_panel == Panel::CommitMessage {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        // ボーダー分を引いた表示可能行数を記録
        self.commit_msg_view_height = area.height.saturating_sub(2);
        let inner_width = area.width.saturating_sub(2);

        let commit_msg = self
            .commit_list_state
            .selected()
            .and_then(|idx| self.commits.get(idx))
            .map(|c| c.commit.message.clone())
            .unwrap_or_default();

        // block なしで line_count を計算（block 付きだとボーダー行が加算されてしまう）
        let paragraph = Paragraph::new(commit_msg).wrap(Wrap { trim: false });

        self.commit_msg_visual_total = paragraph.line_count(inner_width) as u16;
        self.clamp_commit_msg_scroll();

        let block = Block::default()
            .title(" Commit ")
            .borders(Borders::ALL)
            .border_style(border_style);
        let paragraph = paragraph.block(block).scroll((self.commit_msg_scroll, 0));

        frame.render_widget(paragraph, area);

        Self::render_scrollbar(
            frame,
            area,
            self.commit_msg_visual_total as usize,
            self.commit_msg_scroll as usize,
            self.commit_msg_view_height as usize,
        );
    }

    /// Info ペイン描画（PrDescription/Conversation フォーカス時に右上に表示）
    fn render_info_pane(&self, frame: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        // Status (Open/Merged/Closed)
        if !self.pr_state.is_empty() {
            let state_color = match self.pr_state.as_str() {
                "Open" => Color::Green,
                "Merged" => Color::Magenta,
                "Closed" => Color::Red,
                _ => Color::White,
            };
            lines.push(Line::from(vec![
                Span::raw(" Status:  "),
                Span::styled(&self.pr_state, Style::default().fg(state_color)),
            ]));
        }

        // Author
        lines.push(Line::from(vec![
            Span::raw(" Author:  "),
            Span::styled(
                format!("@{}", self.pr_author),
                Style::default().fg(Color::Cyan),
            ),
        ]));

        // Branch
        if !self.pr_base_branch.is_empty() || !self.pr_head_branch.is_empty() {
            lines.push(Line::from(vec![
                Span::raw(" Branch:  "),
                Span::raw(&self.pr_base_branch),
                Span::raw(" ← "),
                Span::styled(&self.pr_head_branch, Style::default().fg(Color::Green)),
            ]));
        }

        // Date
        if !self.pr_created_at.is_empty() {
            lines.push(Line::from(vec![
                Span::raw(" Date:    "),
                Span::raw(&self.pr_created_at),
            ]));
        }

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .title(" Info ")
                .borders(Borders::ALL)
                .border_style(Style::default()),
        );
        frame.render_widget(paragraph, area);
    }

    /// Conversation ペイン描画（PrDescription/Conversation フォーカス時に右中央に表示）
    fn render_conversation_pane(&mut self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focused_panel == Panel::Conversation {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        self.conversation_view_height = area.height.saturating_sub(2);
        let inner_width = area.width.saturating_sub(2);

        self.ensure_conversation_rendered();
        let lines = self.conversation_rendered.as_ref().unwrap().clone();

        let title = format!(" Conversation ({}) ", self.conversation.len());

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        self.conversation_visual_total = paragraph.line_count(inner_width) as u16;
        self.clamp_conversation_scroll();

        let mut block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);
        if self.focused_panel == Panel::Conversation {
            block =
                block.title_bottom(Line::from(HINT_COMMENT).alignment(HorizontalAlignment::Right));
        }
        let paragraph = paragraph.block(block).scroll((self.conversation_scroll, 0));
        frame.render_widget(paragraph, area);

        Self::render_scrollbar(
            frame,
            area,
            self.conversation_visual_total as usize,
            self.conversation_scroll as usize,
            self.conversation_view_height as usize,
        );
    }

    fn render_diff_view_widget(&mut self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focused_panel == Panel::DiffView {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        // DiffView の表示可能サイズを更新（ボーダー分を引く）
        self.diff.view_height = area.height.saturating_sub(2);
        self.diff.view_width = area.width.saturating_sub(2);

        // 選択中ファイルを取得し、所有型にクローンして self の借用を解放
        let (has_file, has_patch, patch, filename, file_status, additions, deletions) = {
            let file = self.current_file();
            let has_file = file.is_some();
            let has_patch = file.is_some_and(|f| f.patch.is_some());
            let patch = file
                .and_then(|f| f.patch.as_deref())
                .unwrap_or("")
                .to_string();
            let filename = file.map(|f| f.filename.as_str()).unwrap_or("").to_string();
            let file_status = file.map(|f| f.status.as_str()).unwrap_or("").to_string();
            let additions = file.map(|f| f.additions).unwrap_or(0);
            let deletions = file.map(|f| f.deletions).unwrap_or(0);
            (
                has_file,
                has_patch,
                patch,
                filename,
                file_status,
                additions,
                deletions,
            )
        };

        // Diff タイトル（左: パス+選択状態, 右: 変更行数）
        let right_title = if has_file && !filename.is_empty() {
            format!(" +{} -{} ", additions, deletions)
        } else {
            String::new()
        };

        let left_title = {
            let selection_suffix = match (&self.mode, &self.line_selection) {
                (AppMode::LineSelect | AppMode::CommentInput, Some(sel)) => {
                    let count = sel.count(self.diff.cursor_line);
                    format!(
                        " - {} line{} selected",
                        count,
                        if count == 1 { "" } else { "s" }
                    )
                }
                _ => String::new(),
            };

            let file_path_part = if has_file && !filename.is_empty() {
                let wrap_width = if self.diff.wrap { 7 } else { 0 }; // " [WRAP]"
                let max_path_width = (area.width as usize)
                    .saturating_sub(2) // borders
                    .saturating_sub(7) // " Diff " + trailing " "
                    .saturating_sub(right_title.len())
                    .saturating_sub(wrap_width)
                    .saturating_sub(selection_suffix.len());
                truncate_path(&filename, max_path_width)
            } else {
                String::new()
            };

            let wrap_suffix = if self.diff.wrap { " [WRAP]" } else { "" };

            if file_path_part.is_empty() {
                if selection_suffix.is_empty() {
                    format!(" Diff{} ", wrap_suffix)
                } else {
                    format!(" Diff{}{} ", selection_suffix, wrap_suffix)
                }
            } else if selection_suffix.is_empty() {
                format!(" Diff {}{} ", file_path_part, wrap_suffix)
            } else {
                format!(
                    " Diff {}{}{} ",
                    file_path_part, selection_suffix, wrap_suffix
                )
            }
        };

        let mut block = Block::default()
            .title(left_title)
            .borders(Borders::ALL)
            .border_style(border_style);
        if !right_title.is_empty() {
            block = block.title_top(Line::from(right_title).alignment(HorizontalAlignment::Right));
        }
        if self.focused_panel == Panel::DiffView
            && !matches!(self.mode, AppMode::CommentInput | AppMode::CommentView)
        {
            let hint = if self.mode == AppMode::LineSelect {
                HINT_COMMENT
            } else {
                HINT_SELECT_COMMENT
            };
            block = block.title_bottom(Line::from(hint).alignment(HorizontalAlignment::Right));
        }

        // バイナリファイルまたは diff がない場合
        if has_file && !has_patch {
            let paragraph = Paragraph::new(Line::styled(
                "Binary file or no diff available",
                Style::default().fg(Color::DarkGray),
            ))
            .block(block);
            frame.render_widget(paragraph, area);
            return;
        }

        let inner_width = area.width.saturating_sub(2);

        self.update_diff_highlight_cache(&patch, &filename, &file_status);
        let mut text = self.prepare_diff_text(&patch, &file_status, inner_width);
        let bg_lines = self.collect_diff_bg_lines(&mut text, &filename);

        // Wrap 有効時、レンダリングに使う実テキストから視覚行オフセットを計算してキャッシュ。
        // visual_line_offset / visual_to_logical_line はこのキャッシュを参照する。
        if self.diff.wrap {
            let mut offsets = Vec::with_capacity(text.lines.len() + 1);
            let mut visual = 0usize;
            offsets.push(0);
            for line in &text.lines {
                let count = Paragraph::new(line.clone())
                    .wrap(Wrap { trim: false })
                    .line_count(inner_width)
                    .max(1);
                visual += count;
                offsets.push(visual);
            }
            self.diff.visual_offsets = Some(offsets);
        } else {
            self.diff.visual_offsets = None;
        }

        let line_count = text.lines.len();
        let paragraph = Paragraph::new(text)
            .block(block)
            .scroll((self.diff.scroll, 0));
        let paragraph = if self.diff.wrap {
            paragraph.wrap(Wrap { trim: false })
        } else {
            paragraph
        };
        frame.render_widget(paragraph, area);

        self.apply_diff_bg_highlights(frame, &bg_lines, area, inner_width);

        let total_visual = self.visual_line_offset(line_count);
        Self::render_scrollbar(
            frame,
            area,
            total_visual,
            self.diff.scroll as usize,
            self.diff.view_height as usize,
        );
    }

    /// delta 出力をキャッシュ（ファイル選択が変わったときだけ再実行）
    fn update_diff_highlight_cache(&mut self, patch: &str, filename: &str, file_status: &str) {
        let commit_idx = self.commit_list_state.selected().unwrap_or(usize::MAX);
        let file_idx = self.file_list_state.selected().unwrap_or(usize::MAX);

        let cache_hit = matches!(
            &self.diff.highlight_cache,
            Some((ci, fi, _)) if *ci == commit_idx && *fi == file_idx
        );

        if !cache_hit {
            let is_whole_file = matches!(file_status, "added" | "removed" | "deleted");
            let base_text = if let Some(highlighted) = highlight_diff(patch, filename, file_status)
            {
                highlighted
            } else {
                // delta 未使用: 手動色分け
                let lines: Vec<Line> = patch
                    .lines()
                    .map(|line| {
                        if is_whole_file {
                            // 全行追加/削除: +/- を除去してデフォルトスタイルで表示
                            let content = if (line.starts_with('+') || line.starts_with('-'))
                                && line.len() > 1
                            {
                                &line[1..]
                            } else if line.starts_with('+') || line.starts_with('-') {
                                ""
                            } else {
                                line
                            };
                            Line::styled(content.to_string(), Style::default())
                        } else {
                            let style = match line.chars().next() {
                                Some('+') => Style::default().fg(Color::Green),
                                Some('-') => Style::default().fg(Color::Red),
                                Some('@') => Style::default().fg(Color::Cyan),
                                _ => Style::default(),
                            };
                            Line::styled(line.to_string(), style)
                        }
                    })
                    .collect();
                Text::from(lines)
            };
            self.diff.highlight_cache = Some((commit_idx, file_idx, base_text));
        }
    }

    /// キャッシュからクローンして Hunk ヘッダー整形・Wrap 空行修正・行番号プレフィックスを適用。
    /// `update_diff_highlight_cache` が事前に呼ばれている必要がある。
    fn prepare_diff_text(&self, patch: &str, file_status: &str, inner_width: u16) -> Text<'static> {
        let mut text = self.diff.highlight_cache.as_ref().unwrap().2.clone();

        // Hunk ヘッダーを整形表示に置換
        let patch_lines: Vec<&str> = patch.lines().collect();

        // delta 出力の余分な末尾行を除去（patch 行数と一致させる）
        text.lines.truncate(patch_lines.len());
        for (idx, line) in text.lines.iter_mut().enumerate() {
            if let Some(raw) = patch_lines.get(idx)
                && raw.starts_with("@@")
            {
                *line = Self::format_hunk_header(raw, inner_width, self.hunk_header_style());
            }
        }

        // Wrap モードで空白のみの行が余分に折り返されるのを防ぐ。
        // ratatui の Paragraph + Wrap { trim: false } は " " を 2 visual rows に展開するため、
        // 空白のみの spans をクリアして空 Line にする（1 visual row でレンダリングされる）。
        if self.diff.wrap {
            for line in &mut text.lines {
                if line.spans.iter().all(|s| s.content.trim().is_empty()) {
                    line.spans.clear();
                }
            }
        }

        // 行番号プレフィックスを各行の先頭に挿入
        if self.diff.show_line_numbers {
            use crate::github::review::parse_hunk_header;

            let line_num_style = Style::default().fg(Color::DarkGray);
            let separator_style = Style::default().fg(Color::DarkGray);
            let mut old_line: usize = 0;
            let mut new_line: usize = 0;

            // 追加/削除ファイルは片側の行番号のみ表示
            let show_old = !matches!(file_status, "added");
            let show_new = !matches!(file_status, "removed" | "deleted");

            for (idx, text_line) in text.lines.iter_mut().enumerate() {
                if let Some(raw) = patch_lines.get(idx) {
                    if raw.starts_with("@@") {
                        // hunk ヘッダー: 行番号をパースして状態更新、表示はなし
                        if let Some((old, new)) = parse_hunk_header(raw) {
                            old_line = old;
                            new_line = new;
                        }
                    } else {
                        let mut prefix = Vec::new();

                        if show_old {
                            let old_str = if raw.starts_with('+') {
                                LINE_NUM_BLANK.to_string()
                            } else {
                                let s = format!("{:>LINE_NUM_WIDTH$} ", old_line);
                                old_line += 1;
                                s
                            };
                            prefix.push(Span::styled(old_str, line_num_style));
                        }

                        if show_new {
                            let new_str = if raw.starts_with('-') {
                                LINE_NUM_BLANK.to_string()
                            } else {
                                let s = format!("{:>LINE_NUM_WIDTH$} ", new_line);
                                new_line += 1;
                                s
                            };
                            prefix.push(Span::styled(new_str, line_num_style));
                        }

                        prefix.push(Span::styled("│", separator_style));
                        text_line.spans.splice(0..0, prefix);
                    }
                }
            }
        }

        text
    }

    /// 既存コメントの下線 / 💬💭 マーカーをテキスト側に適用し、背景色が必要な行を収集。
    /// `filename` は pending コメントのファイルパス照合に使用。
    fn collect_diff_bg_lines(&self, text: &mut Text<'_>, filename: &str) -> Vec<(usize, Color)> {
        let show_cursor = self.focused_panel == Panel::DiffView;
        let has_selection = self.mode == AppMode::LineSelect || self.mode == AppMode::CommentInput;
        let existing_counts = self.existing_comment_counts();
        let cursor_bg = match self.theme {
            ThemeMode::Dark => CURSOR_BG_DARK,
            ThemeMode::Light => CURSOR_BG_LIGHT,
        };
        let pending_bg = match self.theme {
            ThemeMode::Dark => PENDING_BG_DARK,
            ThemeMode::Light => PENDING_BG_LIGHT,
        };

        // 背景色が必要な論理行を収集（render 後に Buffer で適用）
        let mut bg_lines: Vec<(usize, Color)> = Vec::new();

        for (idx, line) in text.lines.iter_mut().enumerate() {
            let is_selected = has_selection
                && self.line_selection.is_some_and(|sel| {
                    let (start, end) = sel.range(self.diff.cursor_line);
                    idx >= start && idx <= end
                });
            let is_cursor = show_cursor && !has_selection && idx == self.diff.cursor_line;
            let is_pending = self
                .review
                .pending_comments
                .iter()
                .any(|c| c.file_path == filename && idx >= c.start_line && idx <= c.end_line);
            let existing_count = existing_counts.get(&idx).copied().unwrap_or(0);

            if is_selected || is_cursor {
                bg_lines.push((idx, cursor_bg));
            } else if is_pending {
                bg_lines.push((idx, pending_bg));
            }

            // 既存コメント行は下線で表示（背景色だとテーマ依存で文字が見えなくなるため）
            if existing_count > 0 && !is_selected && !is_cursor && !is_pending {
                for span in &mut line.spans {
                    span.style = span.style.add_modifier(Modifier::UNDERLINED);
                }
            }

            // 💬 マーカー（既存コメント行の末尾に付与）
            if existing_count > 0 {
                let marker = if existing_count == 1 {
                    " 💬".to_string()
                } else {
                    format!(" 💬{}", existing_count)
                };
                line.spans
                    .push(Span::styled(marker, Style::default().fg(Color::Yellow)));
            }

            // 💭 マーカー（pending コメント行の末尾に付与）
            if is_pending {
                line.spans
                    .push(Span::styled(" 💭", Style::default().fg(Color::Green)));
            }
        }

        bg_lines
    }

    /// Buffer に直接背景色を適用（全幅ハイライト）
    fn apply_diff_bg_highlights(
        &self,
        frame: &mut Frame,
        bg_lines: &[(usize, Color)],
        diff_area: Rect,
        inner_width: u16,
    ) {
        if bg_lines.is_empty() {
            return;
        }
        let inner = Rect {
            x: diff_area.x + 1,
            y: diff_area.y + 1,
            width: inner_width,
            height: diff_area.height.saturating_sub(2),
        };
        let scroll = self.diff.scroll as usize;
        let buf = frame.buffer_mut();
        for &(logical_line, bg_color) in bg_lines {
            let vis_start = self.visual_line_offset(logical_line);
            let vis_end = self.visual_line_offset(logical_line + 1);
            for vis_row in vis_start..vis_end {
                if vis_row < scroll {
                    continue;
                }
                let screen_row = (vis_row - scroll) as u16;
                if screen_row >= inner.height {
                    continue;
                }
                let row_rect = Rect {
                    x: inner.x,
                    y: inner.y + screen_row,
                    width: inner.width,
                    height: 1,
                };
                buf.set_style(row_rect, Style::default().bg(bg_color));
            }
        }
    }

    /// コメント / レビュー本文エディタパネル描画
    /// CommentInput 時は編集可能（緑ボーダー、カーソル表示）、
    /// それ以外は薄いグレーのボーダーで空のコメント欄を表示。
    /// ReviewBodyInput は呼び出し側で全幅パネルとして別途呼び出す。
    fn render_editor_panel(&mut self, frame: &mut Frame, area: Rect) {
        // CommentView モード: viewing_comments をペインに表示（フォーカス状態）
        if self.mode == AppMode::CommentView && !self.review.viewing_comments.is_empty() {
            // render_cursor_comments が &mut self を取るため clone で借用を分離
            let comments = self.review.viewing_comments.clone();
            self.render_cursor_comments(frame, area, &comments, true);
            return;
        }

        // 編集モードでなく Diff が表示中なら、カーソル行のレビューコメントを自動表示
        if !matches!(
            self.mode,
            AppMode::CommentInput | AppMode::IssueCommentInput | AppMode::ReviewBodyInput
        ) && self.layout.diff_view_rect.width > 0
        {
            let comments = self.comments_at_diff_line(self.diff.cursor_line);
            if !comments.is_empty() {
                self.render_cursor_comments(frame, area, &comments, false);
                return;
            }
        }

        let (title, help_text, editor, show_cursor) = match self.mode {
            AppMode::CommentInput => {
                let title = if let Some(selection) = self.line_selection {
                    let (start, end) = selection.range(self.diff.cursor_line);
                    format!(" Comment L{}–L{} ", start + 1, end + 1)
                } else {
                    " Comment ".to_string()
                };
                (
                    title,
                    " Ctrl+G: suggestion | Ctrl+S: submit ",
                    &mut self.review.comment_editor,
                    true,
                )
            }
            AppMode::IssueCommentInput => (
                " Comment (PR) ".to_string(),
                " Ctrl+S: submit ",
                &mut self.review.comment_editor,
                true,
            ),
            AppMode::ReviewBodyInput => {
                let event = self.available_events()[self.review.review_event_cursor];
                (
                    format!(" Review Body ({}) ", event.label()),
                    " Ctrl+S: submit ",
                    &mut self.review.review_body_editor,
                    true,
                )
            }
            _ => (
                " Comment ".to_string(),
                "",
                &mut self.review.comment_editor,
                false,
            ),
        };

        let inner_width = area.width.saturating_sub(2) as usize; // ボーダー左右分
        let visible_height = area.height.saturating_sub(2) as usize; // ボーダー上下分

        editor.set_display_width(inner_width);
        editor.ensure_visible(visible_height);

        let scrollbar_state = editor.scrollbar_state(visible_height);

        let border_style = if show_cursor {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let mut block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);
        if !help_text.is_empty() {
            block = block.title_bottom(Line::from(help_text).alignment(HorizontalAlignment::Right));
        }

        let lines: Vec<Line> = editor
            .lines_from_scroll()
            .iter()
            .map(|l| Line::raw(l.as_str()))
            .collect();

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);

        // Scrollbar（必要な場合のみ）
        if let Some((total_rows, position)) = scrollbar_state {
            Self::render_scrollbar(frame, area, total_rows, position, visible_height);
        }

        // カーソル位置計算（編集中のみ）
        if show_cursor {
            let (vcol, vrow) = editor.cursor_visual_position();
            let cursor_x = area.x + 1 + vcol as u16;
            let cursor_y = area.y + 1 + vrow as u16;
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }

    /// カーソル行のレビューコメントをコメントペインに表示する。
    /// `focused` が true の場合はフォーカス状態（CommentView モード）として描画する。
    fn render_cursor_comments(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        comments: &[crate::github::comments::ReviewComment],
        focused: bool,
    ) {
        // 非フォーカス時はスクロールをリセット（全ナビゲーション経路を統一的にカバー）
        if !focused {
            self.review.viewing_comment_scroll = 0;
        }

        let mut lines = Vec::new();
        for (i, comment) in comments.iter().enumerate() {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(
                format!(
                    "@{} ({})",
                    comment.user.login,
                    format_datetime(&comment.created_at)
                ),
                Style::default().fg(Color::Cyan),
            ));
            for body_line in comment.body.lines() {
                lines.push(Line::raw(body_line.to_string()));
            }
        }

        // ルートコメント ID を特定して resolved 状態を判定
        let is_resolved = crate::github::comments::root_comment_id(comments)
            .and_then(|id| self.review.thread_map.get(&id))
            .is_some_and(|t| t.is_resolved);

        let title = if is_resolved {
            format!(" 💬 Review Comments ({}) [Resolved] ", comments.len())
        } else {
            format!(" 💬 Review Comments ({}) ", comments.len())
        };
        let (help_text, border_color) = if focused {
            let resolve_label = if is_resolved {
                "r: unresolve"
            } else {
                "r: resolve"
            };
            (format!(" {resolve_label} "), Color::Yellow)
        } else {
            (String::new(), Color::DarkGray)
        };
        let mut block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        if !help_text.is_empty() {
            block = block.title_bottom(Line::from(help_text).alignment(HorizontalAlignment::Right));
        }

        // block なしで line_count を計算（block 付きだとボーダー行が加算されてしまう）
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let visible_height = area.height.saturating_sub(2) as usize;
        let inner_width = area.width.saturating_sub(2);
        let visual_total = paragraph.line_count(inner_width);
        self.review.comment_view_max_scroll =
            (visual_total as u16).saturating_sub(visible_height as u16);

        let paragraph = paragraph
            .block(block)
            .scroll((self.review.viewing_comment_scroll, 0));

        frame.render_widget(paragraph, area);

        if visual_total > visible_height {
            Self::render_scrollbar(
                frame,
                area,
                visual_total,
                self.review.viewing_comment_scroll as usize,
                visible_height,
            );
        }
    }

    /// コンテンツがビューポートを超えている場合のみスクロールバーを描画する
    fn render_scrollbar(
        frame: &mut Frame,
        area: Rect,
        total_rows: usize,
        position: usize,
        view_height: usize,
    ) {
        if total_rows <= view_height {
            return;
        }
        let scroll_range = total_rows.saturating_sub(view_height) + 1;
        let mut sb_state = ScrollbarState::new(scroll_range)
            .position(position)
            .viewport_content_length(view_height);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(scrollbar, area, &mut sb_state);
    }

    /// 中央に固定サイズの矩形を配置
    fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        Rect::new(x, y, width.min(area.width), height.min(area.height))
    }

    fn render_review_submit_dialog(&self, frame: &mut Frame, area: Rect) {
        let dialog = Self::centered_rect(REVIEW_DIALOG_WIDTH, REVIEW_DIALOG_HEIGHT, area);
        frame.render_widget(Clear, dialog);

        let comments_info = if self.review.pending_comments.is_empty() {
            "No pending comments".to_string()
        } else {
            format!("{} pending comment(s)", self.review.pending_comments.len())
        };

        let mut lines = vec![Line::raw("")];

        for (i, event) in self.available_events().iter().enumerate() {
            let marker = if i == self.review.review_event_cursor {
                "▶ "
            } else {
                "  "
            };
            let style = if i == self.review.review_event_cursor {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!("{}{}", marker, event.label()), style));
        }

        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("  {}", comments_info),
            Style::default().fg(Color::DarkGray),
        ));

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .title(" Submit Review ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        frame.render_widget(paragraph, dialog);
    }

    fn render_quit_confirm_dialog(&self, frame: &mut Frame, area: Rect) {
        let dialog = Self::centered_rect(QUIT_DIALOG_WIDTH, QUIT_DIALOG_HEIGHT, area);
        frame.render_widget(Clear, dialog);

        let lines = vec![
            Line::raw(""),
            Line::styled(
                format!(
                    "  {} unsent comment(s).",
                    self.review.pending_comments.len()
                ),
                Style::default().fg(Color::Yellow),
            ),
            Line::styled("  Submit before quitting?", Style::default()),
            Line::raw(""),
            Line::styled("  y: submit & quit", Style::default().fg(Color::Green)),
            Line::styled("  n: discard & quit", Style::default().fg(Color::Red)),
            Line::styled("  c: cancel", Style::default().fg(Color::DarkGray)),
        ];

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .title(" Quit Confirmation ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red)),
        );
        frame.render_widget(paragraph, dialog);
    }

    fn render_help_dialog(&mut self, frame: &mut Frame, area: Rect) {
        let dialog_height = (area.height * 2 / 3)
            .max(HELP_DIALOG_MIN_HEIGHT)
            .min(area.height.saturating_sub(4));
        let dialog_width = HELP_DIALOG_WIDTH.min(area.width.saturating_sub(4));
        let dialog = Self::centered_rect(dialog_width, dialog_height, area);
        frame.render_widget(Clear, dialog);

        let s = Style::default().fg(Color::Yellow); // section header
        let k = Style::default().fg(Color::Cyan); // key
        let d = Style::default(); // description
        // ボーダー左右 (2) + インデント (2) + 余白 (2) を引いた幅でセパレータ生成
        let sep_width = (HELP_DIALOG_WIDTH as usize).saturating_sub(6);
        let sep: String = format!("  {}", "─".repeat(sep_width));

        let panel = self.help_context_panel;

        // --- 共通セクション (Global) ---
        let mut entries: Vec<(&str, &str)> = vec![
            ("", "Navigation"),
            ("j / ↓", "Move down"),
            ("k / ↑", "Move up"),
            ("l / → / Tab", "Next pane"),
            ("h / ← / BackTab", "Previous pane"),
            ("1 / 2 / 3 / 4", "Jump to pane"),
            ("Esc", "Back to parent pane"),
            ("z", "Toggle zoom"),
            ("S", "Submit review"),
            ("?", "This help"),
            ("q", "Quit"),
        ];

        // --- Scroll セクション (PrDescription, CommitMessage, Conversation, DiffView) ---
        if matches!(
            panel,
            Panel::PrDescription | Panel::CommitMessage | Panel::Conversation | Panel::DiffView
        ) {
            entries.extend_from_slice(&[
                ("", "Scroll"),
                ("Ctrl+d / Ctrl+u", "Half page down / up"),
                ("Ctrl+f / Ctrl+b", "Full page down / up"),
                ("g / G", "Top / Bottom"),
            ]);
        }

        // --- ペイン固有セクション ---
        match panel {
            Panel::PrDescription => {
                entries.extend_from_slice(&[
                    ("", "PR Description"),
                    ("Enter", "Open conversation"),
                    ("o", "Open media viewer"),
                ]);
            }
            Panel::CommitList => {
                entries.extend_from_slice(&[
                    ("", "Commit List"),
                    ("x", "Toggle viewed"),
                    ("y", "Copy SHA"),
                    ("Y", "Copy commit message"),
                ]);
            }
            Panel::FileTree => {
                entries.extend_from_slice(&[
                    ("", "File Tree"),
                    ("Enter", "Open diff"),
                    ("x", "Toggle viewed"),
                    ("y", "Copy file path"),
                ]);
            }
            Panel::CommitMessage => {
                entries
                    .extend_from_slice(&[("", "Commit Message"), ("Esc", "Back to commit list")]);
            }
            Panel::DiffView => {
                entries.extend_from_slice(&[
                    ("", "Diff View"),
                    ("n", "Toggle line numbers"),
                    ("w", "Toggle line wrap"),
                    ("]c / [c", "Next / prev change block"),
                    ("]h / [h", "Next / prev hunk"),
                    ("]n / [n", "Next / prev comment"),
                    ("v", "Enter line select mode"),
                    ("c", "Comment on line"),
                    ("Enter", "View comment on line"),
                    ("r", "Resolve/unresolve thread"),
                    ("Ctrl+G", "Insert suggestion"),
                    ("Ctrl+S", "Submit comment"),
                ]);
            }
            Panel::Conversation => {
                entries.extend_from_slice(&[
                    ("", "Conversation"),
                    ("c", "Comment on PR"),
                    ("Ctrl+S", "Submit comment"),
                    ("Esc", "Back to PR description"),
                ]);
            }
        }

        let mut lines: Vec<Line> = vec![];
        for (key, desc) in &entries {
            if key.is_empty() {
                // セクションヘッダー
                lines.push(Line::raw(""));
                lines.push(Line::styled(format!("  {desc}"), s));
                lines.push(Line::styled(sep.as_str(), s));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {key:<HELP_KEY_COLUMN_WIDTH$}"), k),
                    Span::styled(*desc, d),
                ]));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "  ?/Esc/q: close",
            Style::default().fg(Color::DarkGray),
        ));

        // コンテンツ末尾を超えてスクロールしないようにクランプ
        let content_height = lines.len() as u16;
        let inner_height = dialog_height.saturating_sub(2); // ボーダー上下分
        let max_scroll = content_height.saturating_sub(inner_height);
        let scroll = self.help_scroll.min(max_scroll);
        // 内部状態も同期して、スクロールアップ時のラグを防ぐ
        self.help_scroll = scroll;

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" Help ({panel}) "))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .scroll((scroll, 0));
        frame.render_widget(paragraph, dialog);
    }

    /// メディアビューアオーバーレイを描画する
    fn render_media_viewer_overlay(&mut self, frame: &mut Frame, area: Rect) {
        // 未キャッシュの画像ならバックグラウンドワーカーを起動
        self.prepare_media_protocol();

        frame.render_widget(Clear, area);

        let total = self.media_count();
        let current = self.media_ref_at(self.media_viewer_index);
        let is_video = current.is_some_and(|r| r.media_type == MediaType::Video);
        let icon = if is_video { "🎬" } else { "🖼" };
        let alt = current.map(|r| r.alt.as_str()).unwrap_or("Media");
        let title = format!(" {icon} {alt} ({}/{total}) ", self.media_viewer_index + 1);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // フッターナビゲーションヒント（inner の最下行）
        let footer_area = Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        );
        let content_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        );

        let k = Style::default().fg(Color::Cyan);
        let footer = Line::from(vec![
            Span::styled(" ← → ", k),
            Span::raw("Navigate  "),
            Span::styled("o ", k),
            Span::raw("Open in browser  "),
            Span::styled("Esc ", k),
            Span::raw("Close"),
        ]);
        frame.render_widget(Paragraph::new(footer), footer_area);

        if is_video {
            let msg = Paragraph::new(
                "🎬 Video cannot be played in terminal\n\nPress o to open in browser",
            )
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Center);
            let centered = Self::centered_rect(45, 3, content_area);
            frame.render_widget(msg, centered);
        } else if let Some(url) = current.map(|r| r.url.clone()) {
            if let Some(protocol) = self.media_protocol_cache.get_mut(&url) {
                let widget = StatefulImage::default();
                frame.render_stateful_widget(widget, content_area, protocol);
            } else if self.media_protocol_worker.is_some() {
                let msg = Paragraph::new("Loading...")
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: false })
                    .alignment(Alignment::Center);
                let centered = Self::centered_rect(15, 1, content_area);
                frame.render_widget(msg, centered);
            } else {
                let msg = Paragraph::new("Press o to open in browser")
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: false });
                let centered = Self::centered_rect(30, 1, content_area);
                frame.render_widget(msg, centered);
            }
        } else {
            let msg = Paragraph::new("Press o to open in browser")
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: false });
            let centered = Self::centered_rect(30, 1, content_area);
            frame.render_widget(msg, centered);
        }
    }
}
