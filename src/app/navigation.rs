use super::*;
use ratatui::widgets::{Paragraph, Wrap};

impl App {
    /// 指定行が hunk header（`@@` で始まる行）かどうか判定
    pub(super) fn is_hunk_header(&self, line_idx: usize) -> bool {
        self.current_file()
            .and_then(|f| f.patch.as_deref())
            .and_then(|p| p.lines().nth(line_idx))
            .is_some_and(|line| line.starts_with("@@"))
    }

    /// hunk header をスキップして次の非 @@ 行に進む（下方向）
    pub(super) fn skip_hunk_header_forward(&self, line: usize, max: usize) -> usize {
        let mut l = line;
        while l < max && self.is_hunk_header(l) {
            l += 1;
        }
        if l >= max { line } else { l }
    }

    /// hunk header をスキップして前の非 @@ 行に戻る（上方向）
    pub(super) fn skip_hunk_header_backward(&self, line: usize, max: usize) -> usize {
        let mut l = line;
        while l > 0 && self.is_hunk_header(l) {
            l -= 1;
        }
        // 行 0 が @@ の場合は下方向にスキップ
        if self.is_hunk_header(l) {
            self.skip_hunk_header_forward(l, max)
        } else {
            l
        }
    }

    /// 2つの diff 行が同一 hunk に属するか判定
    /// hunk header（`@@` で始まる行）を境界として、間に `@@` がなければ同一 hunk
    pub(super) fn is_same_hunk(&self, a: usize, b: usize) -> bool {
        let patch = match self.current_file().and_then(|f| f.patch.as_deref()) {
            Some(p) => p,
            None => return false,
        };
        let lines: Vec<&str> = patch.lines().collect();
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        // lo と hi の間（lo は含まない、hi は含む）に @@ 行があれば別 hunk
        for i in (lo + 1)..=hi {
            if let Some(line) = lines.get(i)
                && line.starts_with("@@")
            {
                return false;
            }
        }
        true
    }

    pub(super) fn select_next(&mut self) {
        match self.focused_panel {
            Panel::PrDescription => {
                self.pr_desc_scroll = self.pr_desc_scroll.saturating_add(1);
                self.clamp_pr_desc_scroll();
            }
            Panel::CommitList if !self.commits.is_empty() => {
                let current = self.commit_list_state.selected().unwrap_or(0);
                let next = (current + 1).min(self.commits.len() - 1);
                self.commit_list_state.select(Some(next));
                if next != current {
                    self.reset_file_selection();
                }
            }
            Panel::FileTree => {
                let files_len = self.current_files().len();
                if files_len > 0 {
                    let current = self.file_list_state.selected().unwrap_or(0);
                    let next = (current + 1).min(files_len - 1);
                    self.file_list_state.select(Some(next));
                    if next != current {
                        self.reset_cursor();
                    }
                }
            }
            Panel::DiffView => {
                self.move_cursor_down();
            }
            _ => {}
        }
    }

    pub(super) fn select_prev(&mut self) {
        match self.focused_panel {
            Panel::PrDescription => {
                self.pr_desc_scroll = self.pr_desc_scroll.saturating_sub(1);
            }
            Panel::CommitList if !self.commits.is_empty() => {
                let current = self.commit_list_state.selected().unwrap_or(0);
                let prev = current.saturating_sub(1);
                self.commit_list_state.select(Some(prev));
                if prev != current {
                    self.reset_file_selection();
                }
            }
            Panel::FileTree => {
                let files_len = self.current_files().len();
                if files_len > 0 {
                    let current = self.file_list_state.selected().unwrap_or(0);
                    let prev = current.saturating_sub(1);
                    self.file_list_state.select(Some(prev));
                    if prev != current {
                        self.reset_cursor();
                    }
                }
            }
            Panel::DiffView => {
                self.move_cursor_up();
            }
            _ => {}
        }
    }

    /// カーソルをリセット（先頭の @@ 行をスキップ）
    pub(super) fn reset_cursor(&mut self) {
        self.diff.cursor_line = 0;
        self.diff.scroll = 0;
        let max = self.current_diff_line_count();
        self.diff.cursor_line = self.skip_hunk_header_forward(0, max);
    }

    /// カーソルを下に移動（@@ 行をスキップ）
    fn move_cursor_down(&mut self) {
        let line_count = self.current_diff_line_count();
        if self.diff.cursor_line + 1 < line_count {
            self.diff.cursor_line += 1;
            self.diff.cursor_line =
                self.skip_hunk_header_forward(self.diff.cursor_line, line_count);
            self.ensure_cursor_visible();
        }
    }

    /// カーソルを上に移動（@@ 行をスキップ）
    fn move_cursor_up(&mut self) {
        if self.diff.cursor_line > 0 {
            self.diff.cursor_line -= 1;
            let max = self.current_diff_line_count();
            self.diff.cursor_line = self.skip_hunk_header_backward(self.diff.cursor_line, max);
            self.ensure_cursor_visible();
        }
    }

    /// 行番号プレフィックスの表示幅を返す
    pub(super) fn line_number_prefix_width(&self) -> u16 {
        if !self.diff.show_line_numbers {
            return 0;
        }
        let file_status = self.current_file().map(|f| f.status.as_str()).unwrap_or("");
        match file_status {
            // 片側のみ: "NNNN │" = 6文字
            "added" | "removed" | "deleted" => 6,
            // 両側: "NNNN NNNN │" = 11文字
            _ => 11,
        }
    }

    /// wrap 有効時に論理行の表示行オフセットを計算する。
    /// 論理行 `logical_line` が始まる表示行番号を返す。
    /// `logical_line == line_count` のとき、合計表示行数を返す。
    /// render 時に計算したキャッシュを優先し、未計算時は patch テキストからフォールバック。
    pub(super) fn visual_line_offset(&self, logical_line: usize) -> usize {
        if !self.diff.wrap {
            return logical_line;
        }
        // キャッシュがあればそれを使う（レンダリングと同じデータソース）
        if let Some(offsets) = &self.diff.visual_offsets {
            return offsets
                .get(logical_line)
                .copied()
                .unwrap_or_else(|| offsets.last().copied().unwrap_or(logical_line));
        }
        // フォールバック: patch テキストから計算（初回 render 前・テスト用）
        let width = self.diff.view_width;
        if width == 0 {
            return logical_line;
        }
        let prefix_width = self.line_number_prefix_width() as usize;
        let patch = match self.current_file().and_then(|f| f.patch.as_deref()) {
            Some(p) => p,
            None => return logical_line,
        };
        let mut visual = 0;
        for (i, line) in patch.lines().enumerate() {
            if i >= logical_line {
                break;
            }
            // @@ 行はプレフィックスなし、それ以外はプレフィックス幅分を加味
            let count = if line.starts_with("@@") || prefix_width == 0 {
                Paragraph::new(line)
                    .wrap(Wrap { trim: false })
                    .line_count(width)
                    .max(1)
            } else {
                let padded = format!("{}{}", " ".repeat(prefix_width), line);
                Paragraph::new(padded.as_str())
                    .wrap(Wrap { trim: false })
                    .line_count(width)
                    .max(1)
            };
            visual += count;
        }
        visual
    }

    /// wrap 有効時に表示行位置から論理行を逆引きする
    pub(super) fn visual_to_logical_line(&self, visual_target: usize) -> usize {
        if !self.diff.wrap {
            return visual_target;
        }
        // キャッシュがあればそれを使う
        if let Some(offsets) = &self.diff.visual_offsets {
            // offsets[i] = 論理行 i の開始表示行。visual_target 以下で最大の i を探す。
            return match offsets.binary_search(&visual_target) {
                Ok(i) => i,
                Err(i) => i.saturating_sub(1),
            };
        }
        // フォールバック: patch テキストから計算
        let width = self.diff.view_width;
        if width == 0 {
            return visual_target;
        }
        let prefix_width = self.line_number_prefix_width() as usize;
        let patch = match self.current_file().and_then(|f| f.patch.as_deref()) {
            Some(p) => p,
            None => return visual_target,
        };
        let mut visual = 0;
        for (i, line) in patch.lines().enumerate() {
            let count = if line.starts_with("@@") || prefix_width == 0 {
                Paragraph::new(line)
                    .wrap(Wrap { trim: false })
                    .line_count(width)
                    .max(1)
            } else {
                let padded = format!("{}{}", " ".repeat(prefix_width), line);
                Paragraph::new(padded.as_str())
                    .wrap(Wrap { trim: false })
                    .line_count(width)
                    .max(1)
            };
            if visual + count > visual_target {
                return i;
            }
            visual += count;
        }
        self.current_diff_line_count().saturating_sub(1)
    }

    /// カーソルが画面内に収まるようスクロールを調整
    pub(super) fn ensure_cursor_visible(&mut self) {
        let visible_lines = self.diff.view_height as usize;
        if visible_lines == 0 {
            return;
        }

        if self.diff.wrap {
            let cursor_visual = self.visual_line_offset(self.diff.cursor_line);
            let cursor_visual_end = self.visual_line_offset(self.diff.cursor_line + 1);
            let scroll = self.diff.scroll as usize;
            if cursor_visual < scroll {
                self.diff.scroll = cursor_visual as u16;
            } else if cursor_visual_end > scroll + visible_lines {
                self.diff.scroll = cursor_visual_end.saturating_sub(visible_lines) as u16;
            }
        } else {
            let scroll = self.diff.scroll as usize;
            if self.diff.cursor_line < scroll {
                self.diff.scroll = self.diff.cursor_line as u16;
            } else if self.diff.cursor_line >= scroll + visible_lines {
                self.diff.scroll = (self.diff.cursor_line - visible_lines + 1) as u16;
            }
        }
    }

    /// 現在の diff の行数を取得
    pub(super) fn current_diff_line_count(&self) -> usize {
        self.current_file()
            .and_then(|f| f.patch.as_ref())
            .map(|p| p.lines().count())
            .unwrap_or(0)
    }

    /// 半ページ下にスクロール（Ctrl+d） — カーソルも追従
    pub(super) fn scroll_diff_down(&mut self) {
        if self.focused_panel != Panel::DiffView {
            return;
        }
        let half = (self.diff.view_height as usize) / 2;
        let line_count = self.current_diff_line_count();
        if self.diff.wrap {
            let target_visual = self.visual_line_offset(self.diff.cursor_line) + half;
            self.diff.cursor_line = self
                .visual_to_logical_line(target_visual)
                .min(line_count.saturating_sub(1));
        } else {
            self.diff.cursor_line =
                (self.diff.cursor_line + half).min(line_count.saturating_sub(1));
        }
        self.diff.cursor_line = self.skip_hunk_header_forward(self.diff.cursor_line, line_count);
        self.ensure_cursor_visible();
    }

    /// 半ページ上にスクロール（Ctrl+u） — カーソルも追従
    pub(super) fn scroll_diff_up(&mut self) {
        if self.focused_panel != Panel::DiffView {
            return;
        }
        let half = (self.diff.view_height as usize) / 2;
        let line_count = self.current_diff_line_count();
        if self.diff.wrap {
            let cur_visual = self.visual_line_offset(self.diff.cursor_line);
            let target_visual = cur_visual.saturating_sub(half);
            self.diff.cursor_line = self.visual_to_logical_line(target_visual);
        } else {
            self.diff.cursor_line = self.diff.cursor_line.saturating_sub(half);
        }
        self.diff.cursor_line = self.skip_hunk_header_backward(self.diff.cursor_line, line_count);
        self.ensure_cursor_visible();
    }

    /// 末尾行にカーソル移動（G）
    pub(super) fn scroll_diff_to_end(&mut self) {
        let line_count = self.current_diff_line_count();
        if line_count > 0 {
            self.diff.cursor_line = line_count - 1;
            self.diff.cursor_line =
                self.skip_hunk_header_backward(self.diff.cursor_line, line_count);
            self.ensure_cursor_visible();
        }
    }

    /// ページ単位で下にスクロール（Ctrl+f）
    pub(super) fn page_down(&mut self) {
        if self.focused_panel != Panel::DiffView {
            return;
        }
        let page = self.diff.view_height as usize;
        let line_count = self.current_diff_line_count();
        if self.diff.wrap {
            let target_visual = self.visual_line_offset(self.diff.cursor_line) + page;
            self.diff.cursor_line = self
                .visual_to_logical_line(target_visual)
                .min(line_count.saturating_sub(1));
        } else {
            self.diff.cursor_line =
                (self.diff.cursor_line + page).min(line_count.saturating_sub(1));
        }
        self.diff.cursor_line = self.skip_hunk_header_forward(self.diff.cursor_line, line_count);
        self.ensure_cursor_visible();
    }

    /// ページ単位で上にスクロール（Ctrl+b）
    pub(super) fn page_up(&mut self) {
        if self.focused_panel != Panel::DiffView {
            return;
        }
        let page = self.diff.view_height as usize;
        let line_count = self.current_diff_line_count();
        if self.diff.wrap {
            let cur_visual = self.visual_line_offset(self.diff.cursor_line);
            let target_visual = cur_visual.saturating_sub(page);
            self.diff.cursor_line = self.visual_to_logical_line(target_visual);
        } else {
            self.diff.cursor_line = self.diff.cursor_line.saturating_sub(page);
        }
        self.diff.cursor_line = self.skip_hunk_header_backward(self.diff.cursor_line, line_count);
        self.ensure_cursor_visible();
    }

    /// 次の変更ブロック（連続する `+`/`-` 行の塊）の先頭にジャンプ
    pub(super) fn jump_to_next_change(&mut self) {
        let patch = match self.current_file().and_then(|f| f.patch.as_deref()) {
            Some(p) => p,
            None => return,
        };
        let lines: Vec<&str> = patch.lines().collect();
        let len = lines.len();
        let mut i = self.diff.cursor_line;

        // 現在の変更ブロック内なら末尾まで飛ばす
        while i < len && Self::is_change_line(lines[i]) {
            i += 1;
        }
        // 非変更行を飛ばす
        while i < len && !Self::is_change_line(lines[i]) {
            i += 1;
        }
        // 次の変更ブロックの先頭に到達
        if i < len {
            self.diff.cursor_line = i;
            self.ensure_cursor_visible();
        }
    }

    /// 前の変更ブロックの先頭にジャンプ
    pub(super) fn jump_to_prev_change(&mut self) {
        let patch = match self.current_file().and_then(|f| f.patch.as_deref()) {
            Some(p) => p,
            None => return,
        };
        let lines: Vec<&str> = patch.lines().collect();
        if self.diff.cursor_line == 0 {
            return;
        }
        let mut i = self.diff.cursor_line - 1;

        // 非変更行を逆方向に飛ばす
        while i > 0 && !Self::is_change_line(lines[i]) {
            i -= 1;
        }
        if !Self::is_change_line(lines[i]) {
            return; // 前方に変更行がない
        }
        // 変更ブロックの先頭を見つける
        while i > 0 && Self::is_change_line(lines[i - 1]) {
            i -= 1;
        }
        self.diff.cursor_line = i;
        self.ensure_cursor_visible();
    }

    pub(super) fn is_change_line(line: &str) -> bool {
        matches!(line.chars().next(), Some('+') | Some('-'))
    }

    /// 次の hunk header（`@@` 行）の次の実コード行にジャンプ
    pub(super) fn jump_to_next_hunk(&mut self) {
        let patch = match self.current_file().and_then(|f| f.patch.as_deref()) {
            Some(p) => p,
            None => return,
        };
        let line_count = patch.lines().count();
        for (i, line) in patch.lines().enumerate().skip(self.diff.cursor_line + 1) {
            if line.starts_with("@@") {
                // @@ の次の実コード行にカーソルを置く
                self.diff.cursor_line = self.skip_hunk_header_forward(i, line_count);
                self.ensure_cursor_visible();
                return;
            }
        }
    }

    /// 前の hunk header（`@@` 行）の次の実コード行にジャンプ
    pub(super) fn jump_to_prev_hunk(&mut self) {
        let patch = match self.current_file().and_then(|f| f.patch.as_deref()) {
            Some(p) => p,
            None => return,
        };
        let lines: Vec<&str> = patch.lines().collect();
        let line_count = lines.len();
        for i in (0..self.diff.cursor_line).rev() {
            if lines[i].starts_with("@@") {
                let target = self.skip_hunk_header_forward(i, line_count);
                // スキップ先が現在位置と同じなら、さらに前の hunk を探す
                if target >= self.diff.cursor_line {
                    continue;
                }
                self.diff.cursor_line = target;
                self.ensure_cursor_visible();
                return;
            }
        }
    }

    pub(super) fn next_panel(&mut self) {
        // DiffView は Tab 巡回の対象外（Enter/Esc で出入りする）
        if self.focused_panel == Panel::DiffView {
            return;
        }
        self.focused_panel = match self.focused_panel {
            Panel::PrDescription => Panel::CommitList,
            Panel::CommitList => Panel::FileTree,
            Panel::FileTree => Panel::PrDescription,
            Panel::DiffView => unreachable!(),
        }
    }
    pub(super) fn prev_panel(&mut self) {
        if self.focused_panel == Panel::DiffView {
            return;
        }
        self.focused_panel = match self.focused_panel {
            Panel::PrDescription => Panel::FileTree,
            Panel::CommitList => Panel::PrDescription,
            Panel::FileTree => Panel::CommitList,
            Panel::DiffView => unreachable!(),
        }
    }
}
