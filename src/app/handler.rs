//! キーボード・マウスイベントのハンドラー関数群

use super::*;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use std::time::Duration;

const EVENT_POLL_MS: u64 = 250;
const HELP_MOUSE_SCROLL_LINES: u16 = 3;

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
                let relative_y = y.saturating_sub(self.layout.commit_list_rect.y + 1);
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
                let relative_y = y.saturating_sub(self.layout.file_tree_rect.y + 1);
                let idx = self.file_list_state.offset() + relative_y as usize;
                if idx < self.current_files().len() {
                    self.file_list_state.select(Some(idx));
                    self.reset_cursor();
                }
            }
            Panel::DiffView => {
                let relative_y = y.saturating_sub(self.layout.diff_view_rect.y + 1);
                if let Some(line) = self.diff_line_at_y(relative_y) {
                    let prev_cursor = self.diff.cursor_line;
                    self.diff.cursor_line = line;
                    if self.diff.cursor_line != prev_cursor {
                        self.review.viewing_comment_scroll = 0;
                    }
                }
            }
            _ => {}
        }
    }

    /// マウスドラッグ処理（DiffView での範囲選択）
    ///
    /// `handle_events` 側でも `focused_panel == DiffView` を確認しているが、
    /// ドラッグ中にポインタが DiffView 外に出た場合のガードとして冒頭でも再チェックする。
    pub(super) fn handle_mouse_drag(&mut self, x: u16, y: u16) {
        // ドラッグ先が DiffView 領域外なら無視
        if self.panel_at(x, y) != Some(Panel::DiffView) {
            return;
        }
        let relative_y = y.saturating_sub(self.layout.diff_view_rect.y + 1);
        let Some(line) = self.diff_line_at_y(relative_y) else {
            return;
        };

        if self.mode == AppMode::Normal && !self.is_hunk_header(self.diff.cursor_line) {
            // ドラッグ開始: 現在のカーソル位置をアンカーとして行選択モードに入る
            self.line_selection = Some(LineSelection {
                anchor: self.diff.cursor_line,
            });
            self.mode = AppMode::LineSelect;
        }

        if self.mode == AppMode::LineSelect
            && let Some(selection) = self.line_selection
            && self.is_same_hunk(selection.anchor, line)
            && !self.is_hunk_header(line)
        {
            self.diff.cursor_line = line;
            self.ensure_cursor_visible();
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
            Panel::CommitMessage => {
                if down {
                    self.commit_msg_scroll = self.commit_msg_scroll.saturating_add(1);
                    self.clamp_commit_msg_scroll();
                } else {
                    self.commit_msg_scroll = self.commit_msg_scroll.saturating_sub(1);
                }
            }
            Panel::Conversation => {
                if down {
                    self.conversation_scroll = self.conversation_scroll.saturating_add(1);
                    self.clamp_conversation_scroll();
                    self.derive_conversation_cursor();
                } else {
                    self.conversation_scroll = self.conversation_scroll.saturating_sub(1);
                    self.derive_conversation_cursor();
                }
            }
            Panel::DiffView => {
                let prev_cursor = self.diff.cursor_line;
                let line_count = self.current_diff_line_count();
                let total_visual = self.visual_line_offset(line_count);
                let max_scroll = (total_visual as u16).saturating_sub(self.diff.view_height);
                if down {
                    if self.diff.scroll < max_scroll {
                        // ビューポートをスクロール + カーソル追従（見た目位置固定）
                        self.diff.scroll += 1;
                        if self.diff.cursor_line + 1 < line_count {
                            self.diff.cursor_line += 1;
                            self.diff.cursor_line =
                                self.skip_hunk_header_forward(self.diff.cursor_line, line_count);
                        }
                    } else if self.diff.cursor_line + 1 < line_count {
                        // ページ末尾に到達 → カーソルのみ移動
                        self.diff.cursor_line += 1;
                        self.diff.cursor_line =
                            self.skip_hunk_header_forward(self.diff.cursor_line, line_count);
                    }
                } else if self.diff.scroll > 0 {
                    self.diff.scroll -= 1;
                    self.diff.cursor_line = self.diff.cursor_line.saturating_sub(1);
                    self.diff.cursor_line =
                        self.skip_hunk_header_backward(self.diff.cursor_line, line_count);
                } else if self.diff.cursor_line > 0 {
                    // ページ先頭に到達 → カーソルのみ移動
                    self.diff.cursor_line -= 1;
                    self.diff.cursor_line =
                        self.skip_hunk_header_backward(self.diff.cursor_line, line_count);
                }
                // カーソル行が変わったらコメントペインのスクロールをリセット
                if self.diff.cursor_line != prev_cursor {
                    self.review.viewing_comment_scroll = 0;
                }
            }
            _ => {}
        }
    }

    /// イベントループからのイベント受信・ディスパッチ
    pub(super) fn handle_events(&mut self) -> Result<()> {
        // 250ms 以内にイベントがなければ早期リターン（render ループを回す）
        if !event::poll(Duration::from_millis(EVENT_POLL_MS))? {
            return Ok(());
        }

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match self.mode {
                AppMode::Normal => self.handle_normal_mode(key.code, key.modifiers),
                AppMode::LineSelect => self.handle_line_select_mode(key.code),
                AppMode::CommentInput => self.handle_comment_input_mode(key.code, key.modifiers),
                AppMode::IssueCommentInput => {
                    self.handle_issue_comment_input_mode(key.code, key.modifiers)
                }
                AppMode::ReplyInput => self.handle_reply_input_mode(key.code, key.modifiers),
                AppMode::CommentView => self.handle_comment_view_mode(key.code),
                AppMode::ReviewSubmit => self.handle_review_submit_mode(key.code),
                AppMode::ReviewBodyInput => {
                    self.handle_review_body_input_mode(key.code, key.modifiers)
                }
                AppMode::QuitConfirm => self.handle_quit_confirm_mode(key.code),
                AppMode::Help => self.handle_help_mode(key.code),
                AppMode::MediaViewer => self.handle_media_viewer_mode(key.code),
            },
            Event::Mouse(mouse) if self.mode == AppMode::Help => match mouse.kind {
                MouseEventKind::ScrollDown => {
                    self.help_scroll = self.help_scroll.saturating_add(HELP_MOUSE_SCROLL_LINES);
                }
                MouseEventKind::ScrollUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(HELP_MOUSE_SCROLL_LINES);
                }
                _ => {}
            },
            Event::Mouse(mouse)
                if self.mode == AppMode::Normal || self.mode == AppMode::LineSelect =>
            {
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) if self.mode == AppMode::Normal => {
                        self.handle_mouse_click(mouse.column, mouse.row);
                    }
                    MouseEventKind::Drag(MouseButton::Left)
                        if self.focused_panel == Panel::DiffView =>
                    {
                        self.handle_mouse_drag(mouse.column, mouse.row);
                    }
                    MouseEventKind::ScrollDown if self.mode == AppMode::Normal => {
                        self.handle_mouse_scroll(mouse.column, mouse.row, true);
                    }
                    MouseEventKind::ScrollUp if self.mode == AppMode::Normal => {
                        self.handle_mouse_scroll(mouse.column, mouse.row, false);
                    }
                    _ => {}
                }
            }
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
                    (']', KeyCode::Char('n')) => self.jump_to_next_comment(),
                    ('[', KeyCode::Char('n')) => self.jump_to_prev_comment(),
                    _ => {} // 不明な2文字目は無視
                }
            }
            return;
        }

        if self.handle_global_keys(code, modifiers) {
            return;
        }

        match self.focused_panel {
            Panel::PrDescription => self.handle_pr_desc_keys(code),
            Panel::CommitList => self.handle_commit_list_keys(code),
            Panel::FileTree => self.handle_file_tree_keys(code),
            Panel::CommitMessage => self.handle_commit_msg_keys(code),
            Panel::DiffView => self.handle_diff_view_keys(code),
            Panel::Conversation => self.handle_conversation_keys(code),
        }
    }

    /// パネル共通のキー処理（処理した場合 true を返す）
    fn handle_global_keys(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
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
            KeyCode::Char('4') => self.focused_panel = Panel::CommitMessage,
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                match self.focused_panel {
                    Panel::PrDescription => {
                        let half = self.pr_desc_view_height / 2;
                        self.pr_desc_scroll = self.pr_desc_scroll.saturating_add(half);
                        self.clamp_pr_desc_scroll();
                    }
                    Panel::CommitMessage => {
                        let half = self.commit_msg_view_height / 2;
                        self.commit_msg_scroll = self.commit_msg_scroll.saturating_add(half);
                        self.clamp_commit_msg_scroll();
                    }
                    Panel::Conversation => {
                        let half = self.conversation_view_height / 2;
                        self.conversation_scroll = self.conversation_scroll.saturating_add(half);
                        self.clamp_conversation_scroll();
                        self.derive_conversation_cursor();
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
                    Panel::CommitMessage => {
                        let half = self.commit_msg_view_height / 2;
                        self.commit_msg_scroll = self.commit_msg_scroll.saturating_sub(half);
                    }
                    Panel::Conversation => {
                        let half = self.conversation_view_height / 2;
                        self.conversation_scroll = self.conversation_scroll.saturating_sub(half);
                        self.derive_conversation_cursor();
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
                    Panel::CommitMessage => {
                        self.commit_msg_scroll = self
                            .commit_msg_scroll
                            .saturating_add(self.commit_msg_view_height);
                        self.clamp_commit_msg_scroll();
                    }
                    Panel::Conversation => {
                        self.conversation_scroll = self
                            .conversation_scroll
                            .saturating_add(self.conversation_view_height);
                        self.clamp_conversation_scroll();
                        self.derive_conversation_cursor();
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
                    Panel::CommitMessage => {
                        self.commit_msg_scroll = self
                            .commit_msg_scroll
                            .saturating_sub(self.commit_msg_view_height);
                    }
                    Panel::Conversation => {
                        self.conversation_scroll = self
                            .conversation_scroll
                            .saturating_sub(self.conversation_view_height);
                        self.derive_conversation_cursor();
                    }
                    _ => self.page_up(),
                }
            }
            KeyCode::Char('g') => match self.focused_panel {
                Panel::PrDescription => {
                    self.pr_desc_scroll = 0;
                }
                Panel::CommitMessage => {
                    self.commit_msg_scroll = 0;
                }
                Panel::Conversation => {
                    self.conversation_cursor = 0;
                    self.conversation_scroll = 0;
                }
                Panel::DiffView => {
                    self.diff.cursor_line = 0;
                    self.diff.scroll = 0;
                    let max = self.current_diff_line_count();
                    self.diff.cursor_line = self.skip_hunk_header_forward(0, max);
                    self.review.viewing_comment_scroll = 0;
                }
                _ => {}
            },
            KeyCode::Char('G') => match self.focused_panel {
                Panel::PrDescription => {
                    self.pr_desc_scroll = self.pr_desc_max_scroll();
                }
                Panel::CommitMessage => {
                    self.commit_msg_scroll = self.commit_msg_max_scroll();
                }
                Panel::Conversation => {
                    self.conversation_cursor = self.conversation.len().saturating_sub(1);
                    self.conversation_scroll = self.conversation_max_scroll();
                }
                Panel::DiffView => {
                    self.scroll_diff_to_end();
                    self.review.viewing_comment_scroll = 0;
                }
                _ => {}
            },
            KeyCode::Char('S') => {
                self.review.review_event_cursor = 0;
                self.mode = AppMode::ReviewSubmit;
            }
            KeyCode::Char('w') => {
                if self.diff.wrap {
                    // ON → OFF: 表示行→論理行に変換
                    let logical = self.visual_to_logical_line(self.diff.scroll as usize);
                    self.diff.wrap = false;
                    self.diff.scroll = logical as u16;
                } else {
                    // OFF → ON: 論理行→表示行に変換
                    let visual = self.visual_line_offset(self.diff.scroll as usize);
                    self.diff.wrap = true;
                    self.diff.scroll = visual as u16;
                }
                // 次の render で再計算されるまでの1フレームの不整合を防ぐ
                self.diff.visual_offsets = None;
                self.ensure_cursor_visible();
            }
            KeyCode::Char('n') => {
                self.diff.show_line_numbers = !self.diff.show_line_numbers;
                self.diff.visual_offsets = None;
                self.ensure_cursor_visible();
            }
            KeyCode::Char('z') => {
                self.zoomed = !self.zoomed;
                // zoom 切替で描画幅が変わり、Wrap 済み視覚行数も変わる
                self.pr_desc_visual_total = 0;
                self.commit_msg_visual_total = 0;
                self.conversation_visual_total = 0;
            }
            KeyCode::Char('R') => {
                if self.needs_reload {
                    // リロード中は無視
                } else if !self.review.pending_comments.is_empty() {
                    self.status_message = Some(StatusMessage::error(
                        "✗ Cannot reload with pending comments. Submit or discard first.",
                    ));
                } else {
                    self.needs_reload = true;
                }
            }
            KeyCode::Char('?') => {
                self.help_scroll = 0;
                self.help_context_panel = self.focused_panel;
                self.mode = AppMode::Help;
            }
            KeyCode::Char(ch @ (']' | '[')) => {
                self.pending_key = Some(ch);
            }
            _ => return false,
        }
        true
    }

    /// PR Description パネルのキー処理
    fn handle_pr_desc_keys(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                self.focused_panel = Panel::Conversation;
            }
            KeyCode::Char('o') => {
                self.enter_media_viewer();
            }
            _ => {}
        }
    }

    /// Commit List パネルのキー処理
    fn handle_commit_list_keys(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('x') => self.toggle_commit_viewed(),
            KeyCode::Char('y') => {
                if let Some(idx) = self.commit_list_state.selected()
                    && let Some(commit) = self.commits.get(idx)
                {
                    let sha = commit.short_sha().to_string();
                    self.copy_to_clipboard(&sha, "SHA");
                }
            }
            KeyCode::Char('Y') => {
                if let Some(idx) = self.commit_list_state.selected()
                    && let Some(commit) = self.commits.get(idx)
                {
                    let msg = commit.message_summary().to_string();
                    self.copy_to_clipboard(&msg, "message");
                }
            }
            _ => {}
        }
    }

    /// File Tree パネルのキー処理
    fn handle_file_tree_keys(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => self.focused_panel = Panel::DiffView,
            KeyCode::Char('x') => self.toggle_viewed(),
            KeyCode::Char('y') => {
                if let Some(file) = self.current_file() {
                    let path = file.filename.clone();
                    self.copy_to_clipboard(&path, "path");
                }
            }
            _ => {}
        }
    }

    /// DiffView パネルのキー処理
    fn handle_diff_view_keys(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                // DiffView で Enter → カーソル行にコメントがあれば CommentView
                let comments = self.comments_at_diff_line(self.diff.cursor_line);
                if !comments.is_empty() {
                    self.review.viewing_comments = comments;
                    self.mode = AppMode::CommentView;
                }
            }
            KeyCode::Esc => {
                // DiffView で Esc → Files に戻る
                self.focused_panel = Panel::FileTree;
            }
            KeyCode::Char('v') => {
                // DiffView パネルでのみ行選択モードに入る
                self.enter_line_select_mode();
            }
            KeyCode::Char('c') => {
                // DiffView で直接 c: カーソル行のみで単一行コメント（hunk header 上は不可）
                if !self.is_hunk_header(self.diff.cursor_line) {
                    self.line_selection = Some(LineSelection {
                        anchor: self.diff.cursor_line,
                    });
                    self.review.comment_editor.clear();
                    self.mode = AppMode::CommentInput;
                }
            }
            _ => {}
        }
    }

    /// Commit Message パネルのキー処理
    fn handle_commit_msg_keys(&mut self, code: KeyCode) {
        if code == KeyCode::Esc {
            self.focused_panel = Panel::CommitList;
        }
    }

    /// Conversation パネルのキー処理
    fn handle_conversation_keys(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.focused_panel = Panel::PrDescription;
            }
            KeyCode::Char('c') => {
                // カーソル位置のエントリが CodeComment なら返信、それ以外なら新規 issue comment
                if let Some(entry) = self.conversation.get(self.conversation_cursor) {
                    if let ConversationKind::CodeComment {
                        root_comment_id, ..
                    } = entry.kind
                    {
                        self.review.reply_to_comment_id = Some(root_comment_id);
                        self.review.comment_editor.clear();
                        self.mode = AppMode::ReplyInput;
                        return;
                    }
                }
                self.review.comment_editor.clear();
                self.mode = AppMode::IssueCommentInput;
            }
            _ => {}
        }
    }

    /// 返信入力モードのキー処理
    pub(super) fn handle_reply_input_mode(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Esc => {
                self.review.comment_editor.clear();
                self.review.reply_to_comment_id = None;
                // CommentView から入った場合（viewing_comments が残っている）は CommentView に戻る
                if !self.review.viewing_comments.is_empty() {
                    self.mode = AppMode::CommentView;
                } else {
                    self.mode = AppMode::Normal;
                }
                return;
            }
            KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                let text = self.review.comment_editor.text();
                if text.trim().is_empty() {
                    self.status_message = Some(StatusMessage::error("Reply is empty"));
                    return;
                }
                self.needs_reply_submit = true;
                self.mode = AppMode::Normal;
                return;
            }
            _ => {
                self.review.comment_editor.handle_key(code, modifiers);
            }
        }
        self.review
            .comment_editor
            .ensure_visible(editor::EDITOR_VISIBLE_HEIGHT);
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
    pub(super) fn handle_comment_input_mode(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Esc => self.cancel_comment_input(),
            KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.confirm_comment();
            }
            KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_suggestion();
            }
            _ => {
                self.review.comment_editor.handle_key(code, modifiers);
            }
        }
        self.review
            .comment_editor
            .ensure_visible(editor::EDITOR_VISIBLE_HEIGHT);
    }

    /// Issue Comment 入力モードのキー処理
    pub(super) fn handle_issue_comment_input_mode(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) {
        match code {
            KeyCode::Esc => {
                self.review.comment_editor.clear();
                self.mode = AppMode::Normal;
                self.focused_panel = Panel::Conversation;
                return;
            }
            KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                let text = self.review.comment_editor.text();
                if text.trim().is_empty() {
                    self.status_message = Some(StatusMessage::error("Comment is empty"));
                    return;
                }
                self.needs_issue_comment_submit = true;
                self.mode = AppMode::Normal;
                self.focused_panel = Panel::Conversation;
                return;
            }
            _ => {
                self.review.comment_editor.handle_key(code, modifiers);
            }
        }
        self.review
            .comment_editor
            .ensure_visible(editor::EDITOR_VISIBLE_HEIGHT);
    }

    /// コメントペイン（フォーカス状態）のキー処理
    pub(super) fn handle_comment_view_mode(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
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
            KeyCode::Char('r') => {
                self.toggle_resolve_thread();
            }
            KeyCode::Char('c') => {
                // viewing_comments からルートコメント ID を取得して返信モードへ
                if let Some(root_id) =
                    crate::github::comments::root_comment_id(&self.review.viewing_comments)
                {
                    self.review.reply_to_comment_id = Some(root_id);
                    self.review.comment_editor.clear();
                    self.mode = AppMode::ReplyInput;
                }
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
                self.review.review_body_editor.clear();
                self.mode = AppMode::ReviewBodyInput;
            }
            _ => {}
        }
    }

    /// レビュー本文入力モードのキー処理
    pub(super) fn handle_review_body_input_mode(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Esc => {
                self.review.review_body_editor.clear();
                self.mode = AppMode::ReviewSubmit;
            }
            KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                let event = self.available_events()[self.review.review_event_cursor];
                self.review.needs_submit = Some(event);
                self.mode = AppMode::Normal;
            }
            _ => {
                self.review.review_body_editor.handle_key(code, modifiers);
            }
        }
        self.review
            .review_body_editor
            .ensure_visible(editor::EDITOR_VISIBLE_HEIGHT);
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
