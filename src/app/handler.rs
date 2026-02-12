//! キーボード・マウスイベントのハンドラー関数群

use super::*;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use std::time::Duration;

impl App {
    /// マウスクリック処理
    pub(super) fn handle_mouse_click(&mut self, x: u16, y: u16) {
        let Some(panel) = self.panel_at(x, y) else {
            return;
        };
        self.focused_panel = panel;

        // リスト内アイテムのクリック選択
        match panel {
            Panel::CommitList => {
                let relative_y = y.saturating_sub(self.commit_list_rect.y + 1);
                let idx = self.commit_list_state.offset() + relative_y as usize;
                if idx < self.commits.len() {
                    let old = self.commit_list_state.selected();
                    self.commit_list_state.select(Some(idx));
                    if old != Some(idx) {
                        self.reset_file_selection();
                    }
                }
            }
            Panel::FileTree => {
                let relative_y = y.saturating_sub(self.file_tree_rect.y + 1);
                let idx = self.file_list_state.offset() + relative_y as usize;
                if idx < self.current_files().len() {
                    self.file_list_state.select(Some(idx));
                    self.reset_cursor();
                }
            }
            _ => {}
        }
    }

    /// マウススクロール処理（PR Description と DiffView のみ）
    pub(super) fn handle_mouse_scroll(&mut self, x: u16, y: u16, down: bool) {
        let Some(panel) = self.panel_at(x, y) else {
            return;
        };
        match panel {
            Panel::PrDescription => {
                if down {
                    self.pr_desc_scroll = self.pr_desc_scroll.saturating_add(1);
                    self.clamp_pr_desc_scroll();
                } else {
                    self.pr_desc_scroll = self.pr_desc_scroll.saturating_sub(1);
                }
            }
            Panel::DiffView => {
                let line_count = self.current_diff_line_count();
                let total_visual = self.visual_line_offset(line_count);
                let max_scroll = (total_visual as u16).saturating_sub(self.diff_view_height);
                if down {
                    if self.diff_scroll < max_scroll {
                        // ビューポートをスクロール + カーソル追従（見た目位置固定）
                        self.diff_scroll += 1;
                        if self.cursor_line + 1 < line_count {
                            self.cursor_line += 1;
                            self.cursor_line =
                                self.skip_hunk_header_forward(self.cursor_line, line_count);
                        }
                    } else if self.cursor_line + 1 < line_count {
                        // ページ末尾に到達 → カーソルのみ移動
                        self.cursor_line += 1;
                        self.cursor_line =
                            self.skip_hunk_header_forward(self.cursor_line, line_count);
                    }
                } else if self.diff_scroll > 0 {
                    self.diff_scroll -= 1;
                    self.cursor_line = self.cursor_line.saturating_sub(1);
                    self.cursor_line = self.skip_hunk_header_backward(self.cursor_line, line_count);
                } else if self.cursor_line > 0 {
                    // ページ先頭に到達 → カーソルのみ移動
                    self.cursor_line -= 1;
                    self.cursor_line = self.skip_hunk_header_backward(self.cursor_line, line_count);
                }
            }
            _ => {}
        }
    }

    /// イベントループからのイベント受信・ディスパッチ
    pub(super) fn handle_events(&mut self) -> Result<()> {
        // 250ms 以内にイベントがなければ早期リターン（render ループを回す）
        if !event::poll(Duration::from_millis(250))? {
            return Ok(());
        }

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match self.mode {
                AppMode::Normal => self.handle_normal_mode(key.code, key.modifiers),
                AppMode::LineSelect => self.handle_line_select_mode(key.code),
                AppMode::CommentInput => self.handle_comment_input_mode(key.code),
                AppMode::CommentView => self.handle_comment_view_mode(key.code),
                AppMode::ReviewSubmit => self.handle_review_submit_mode(key.code),
                AppMode::ReviewBodyInput => self.handle_review_body_input_mode(key.code),
                AppMode::QuitConfirm => self.handle_quit_confirm_mode(key.code),
                AppMode::Help => self.handle_help_mode(key.code),
                AppMode::MediaViewer => self.handle_media_viewer_mode(key.code),
            },
            Event::Mouse(mouse) if self.mode == AppMode::Normal => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.handle_mouse_click(mouse.column, mouse.row);
                }
                MouseEventKind::ScrollDown => {
                    self.handle_mouse_scroll(mouse.column, mouse.row, true);
                }
                MouseEventKind::ScrollUp => {
                    self.handle_mouse_scroll(mouse.column, mouse.row, false);
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    /// 通常モードのキー処理
    pub(super) fn handle_normal_mode(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        // 2キーシーケンスの処理（] or [ の後の2文字目）
        if let Some(first) = self.pending_key.take() {
            if self.focused_panel == Panel::DiffView {
                match (first, &code) {
                    (']', KeyCode::Char('c')) => self.jump_to_next_change(),
                    ('[', KeyCode::Char('c')) => self.jump_to_prev_change(),
                    (']', KeyCode::Char('h')) => self.jump_to_next_hunk(),
                    ('[', KeyCode::Char('h')) => self.jump_to_prev_hunk(),
                    _ => {} // 不明な2文字目は無視
                }
            }
            return;
        }

        match code {
            KeyCode::Char('q') => {
                if self.review.pending_comments.is_empty() {
                    self.should_quit = true;
                } else {
                    self.mode = AppMode::QuitConfirm;
                }
            }
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => self.next_panel(),
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => self.prev_panel(),
            // 数字キーでペイン直接ジャンプ
            KeyCode::Char('1') => self.focused_panel = Panel::PrDescription,
            KeyCode::Char('2') => self.focused_panel = Panel::CommitList,
            KeyCode::Char('3') => self.focused_panel = Panel::FileTree,
            KeyCode::Enter => {
                if self.focused_panel == Panel::PrDescription {
                    // PR Description で Enter → 画像があれば ImageViewer
                    self.enter_media_viewer();
                } else if self.focused_panel == Panel::FileTree {
                    // Files ペインで Enter → DiffView に移動
                    self.focused_panel = Panel::DiffView;
                } else if self.focused_panel == Panel::DiffView {
                    // DiffView で Enter → カーソル行にコメントがあれば CommentView
                    let comments = self.comments_at_diff_line(self.cursor_line);
                    if !comments.is_empty() {
                        self.review.viewing_comments = comments;
                        self.mode = AppMode::CommentView;
                    }
                }
            }
            KeyCode::Esc => {
                // DiffView で Esc → Files に戻る
                if self.focused_panel == Panel::DiffView {
                    self.focused_panel = Panel::FileTree;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                match self.focused_panel {
                    Panel::PrDescription => {
                        let half = self.pr_desc_view_height / 2;
                        self.pr_desc_scroll = self.pr_desc_scroll.saturating_add(half);
                        self.clamp_pr_desc_scroll();
                    }
                    _ => self.scroll_diff_down(),
                }
            }
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                match self.focused_panel {
                    Panel::PrDescription => {
                        let half = self.pr_desc_view_height / 2;
                        self.pr_desc_scroll = self.pr_desc_scroll.saturating_sub(half);
                    }
                    _ => self.scroll_diff_up(),
                }
            }
            KeyCode::Char('f') if modifiers.contains(KeyModifiers::CONTROL) => {
                match self.focused_panel {
                    Panel::PrDescription => {
                        self.pr_desc_scroll =
                            self.pr_desc_scroll.saturating_add(self.pr_desc_view_height);
                        self.clamp_pr_desc_scroll();
                    }
                    _ => self.page_down(),
                }
            }
            KeyCode::Char('b') if modifiers.contains(KeyModifiers::CONTROL) => {
                match self.focused_panel {
                    Panel::PrDescription => {
                        self.pr_desc_scroll =
                            self.pr_desc_scroll.saturating_sub(self.pr_desc_view_height);
                    }
                    _ => self.page_up(),
                }
            }
            KeyCode::Char('g') => match self.focused_panel {
                Panel::PrDescription => {
                    self.pr_desc_scroll = 0;
                }
                Panel::DiffView => {
                    self.cursor_line = 0;
                    self.diff_scroll = 0;
                    let max = self.current_diff_line_count();
                    self.cursor_line = self.skip_hunk_header_forward(0, max);
                }
                _ => {}
            },
            KeyCode::Char('G') => match self.focused_panel {
                Panel::PrDescription => {
                    self.pr_desc_scroll = self.pr_desc_max_scroll();
                }
                Panel::DiffView => {
                    self.scroll_diff_to_end();
                }
                _ => {}
            },
            KeyCode::Char('v') => {
                // DiffView パネルでのみ行選択モードに入る
                if self.focused_panel == Panel::DiffView {
                    self.enter_line_select_mode();
                }
            }
            KeyCode::Char('c') => {
                // DiffView で直接 c: カーソル行のみで単一行コメント（hunk header 上は不可）
                if self.focused_panel == Panel::DiffView && !self.is_hunk_header(self.cursor_line) {
                    self.line_selection = Some(LineSelection {
                        anchor: self.cursor_line,
                    });
                    self.review.comment_input.clear();
                    self.mode = AppMode::CommentInput;
                }
            }
            KeyCode::Char('x') => match self.focused_panel {
                Panel::FileTree => self.toggle_viewed(),
                Panel::CommitList => self.toggle_commit_viewed(),
                _ => {}
            },
            KeyCode::Char('y') => match self.focused_panel {
                Panel::CommitList => {
                    if let Some(idx) = self.commit_list_state.selected()
                        && let Some(commit) = self.commits.get(idx)
                    {
                        let sha = commit.short_sha().to_string();
                        self.copy_to_clipboard(&sha, "SHA");
                    }
                }
                Panel::FileTree => {
                    if let Some(file) = self.current_file() {
                        let path = file.filename.clone();
                        self.copy_to_clipboard(&path, "path");
                    }
                }
                _ => {}
            },
            KeyCode::Char('Y') => {
                if self.focused_panel == Panel::CommitList
                    && let Some(idx) = self.commit_list_state.selected()
                    && let Some(commit) = self.commits.get(idx)
                {
                    let msg = commit.message_summary().to_string();
                    self.copy_to_clipboard(&msg, "message");
                }
            }
            KeyCode::Char('S') => {
                self.review.review_event_cursor = 0;
                self.mode = AppMode::ReviewSubmit;
            }
            KeyCode::Char('w') => {
                if self.diff_wrap {
                    // ON → OFF: 表示行→論理行に変換
                    let logical = self.visual_to_logical_line(self.diff_scroll as usize);
                    self.diff_wrap = false;
                    self.diff_scroll = logical as u16;
                } else {
                    // OFF → ON: 論理行→表示行に変換
                    let visual = self.visual_line_offset(self.diff_scroll as usize);
                    self.diff_wrap = true;
                    self.diff_scroll = visual as u16;
                }
                // 次の render で再計算されるまでの1フレームの不整合を防ぐ
                self.diff_visual_offsets = None;
                self.ensure_cursor_visible();
            }
            KeyCode::Char('n') => {
                self.show_line_numbers = !self.show_line_numbers;
                self.diff_visual_offsets = None;
                self.ensure_cursor_visible();
            }
            KeyCode::Char('z') => {
                self.zoomed = !self.zoomed;
                // zoom 切替で描画幅が変わり、Wrap 済み視覚行数も変わる
                self.pr_desc_visual_total = 0;
            }
            KeyCode::Char('?') => {
                self.help_scroll = 0;
                self.mode = AppMode::Help;
            }
            KeyCode::Char(']') | KeyCode::Char('[') => {
                if let KeyCode::Char(ch) = code {
                    self.pending_key = Some(ch);
                }
            }
            _ => {}
        }
    }

    /// 行選択モードのキー処理
    pub(super) fn handle_line_select_mode(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => self.exit_line_select_mode(),
            KeyCode::Char('j') | KeyCode::Down => self.extend_selection_down(),
            KeyCode::Char('k') | KeyCode::Up => self.extend_selection_up(),
            KeyCode::Char('c') => self.enter_comment_input_mode(),
            _ => {}
        }
    }

    /// コメント入力モードのキー処理
    pub(super) fn handle_comment_input_mode(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => self.cancel_comment_input(),
            KeyCode::Enter => self.confirm_comment(),
            KeyCode::Backspace => {
                self.review.comment_input.pop();
            }
            KeyCode::Char(c) => {
                self.review.comment_input.push(c);
            }
            _ => {}
        }
    }

    /// コメント表示ダイアログのキー処理
    pub(super) fn handle_comment_view_mode(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                self.review.viewing_comments.clear();
                self.review.viewing_comment_scroll = 0;
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.review.viewing_comment_scroll < self.review.comment_view_max_scroll {
                    self.review.viewing_comment_scroll += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.review.viewing_comment_scroll =
                    self.review.viewing_comment_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    /// レビュー送信ダイアログのキー処理
    pub(super) fn handle_review_submit_mode(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.review.quit_after_submit = false;
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.review.review_event_cursor =
                    (self.review.review_event_cursor + 1) % self.available_events().len();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.review.review_event_cursor = if self.review.review_event_cursor == 0 {
                    self.available_events().len() - 1
                } else {
                    self.review.review_event_cursor - 1
                };
            }
            KeyCode::Enter => {
                let event = self.available_events()[self.review.review_event_cursor];
                // COMMENT は pending_comments が必要
                if event == ReviewEvent::Comment && self.review.pending_comments.is_empty() {
                    self.status_message =
                        Some(StatusMessage::error("No pending comments to submit"));
                    self.mode = AppMode::Normal;
                    return;
                }
                self.review.review_body_input.clear();
                self.mode = AppMode::ReviewBodyInput;
            }
            _ => {}
        }
    }

    /// レビュー本文入力モードのキー処理
    pub(super) fn handle_review_body_input_mode(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.review.review_body_input.clear();
                self.mode = AppMode::ReviewSubmit;
            }
            KeyCode::Enter => {
                let event = self.available_events()[self.review.review_event_cursor];
                self.status_message = Some(StatusMessage::info(format!(
                    "Submitting ({})...",
                    event.label()
                )));
                self.review.needs_submit = Some(event);
                self.mode = AppMode::Normal;
            }
            KeyCode::Backspace => {
                self.review.review_body_input.pop();
            }
            KeyCode::Char(c) => {
                self.review.review_body_input.push(c);
            }
            _ => {}
        }
    }

    /// 終了確認ダイアログのキー処理
    pub(super) fn handle_quit_confirm_mode(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('y') => {
                // レビュー送信ダイアログへ遷移（送信後に終了）
                self.review.review_event_cursor = 0;
                self.review.quit_after_submit = true;
                self.mode = AppMode::ReviewSubmit;
            }
            KeyCode::Char('n') => {
                // 破棄して終了
                self.review.pending_comments.clear();
                self.should_quit = true;
            }
            KeyCode::Char('c') | KeyCode::Esc => {
                // キャンセル
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    /// ヘルプ表示モードのキー処理
    pub(super) fn handle_help_mode(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.help_scroll = self.help_scroll.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_scroll = self.help_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    /// メディアビューアーモードのキー処理
    pub(super) fn handle_media_viewer_mode(&mut self, code: KeyCode) {
        let count = self.media_count();
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.media_viewer_protocol = None;
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if count > 0 {
                    self.media_viewer_index = (self.media_viewer_index + 1) % count;
                    self.prepare_media_protocol();
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if count > 0 {
                    self.media_viewer_index = if self.media_viewer_index == 0 {
                        count - 1
                    } else {
                        self.media_viewer_index - 1
                    };
                    self.prepare_media_protocol();
                }
            }
            KeyCode::Char('o') => {
                if let Some(url) = self
                    .media_ref_at(self.media_viewer_index)
                    .map(|r| r.url.clone())
                {
                    open_url_in_browser(&url);
                }
            }
            _ => {}
        }
    }
}
