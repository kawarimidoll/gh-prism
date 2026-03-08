use crossterm::event::{KeyCode, KeyModifiers};
use unicode_width::UnicodeWidthChar;

/// エディタの表示可能行数（CommentInput / ReviewBodyInput 共通）
pub const EDITOR_VISIBLE_HEIGHT: usize = 5;

/// 複数行テキストエディタ
#[derive(Debug)]
pub struct TextEditor {
    lines: Vec<String>,
    cursor_row: usize,
    /// カーソル位置（バイトオフセット）
    cursor_col: usize,
    scroll_offset: usize,
    /// 最後に設定された表示幅（wrap 計算用、0 = wrap 無効）
    display_width: usize,
}

impl Default for TextEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEditor {
    /// 空の1行で初期化
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            display_width: 0,
        }
    }

    /// テキストを設定（clear + insert_text）
    pub fn set_text(&mut self, text: &str) {
        self.clear();
        self.insert_text(text);
    }

    /// 初期状態にリセット
    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_offset = 0;
    }

    /// 全行が空か判定
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    /// `\n` 結合で String を返す
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// 表示幅を設定する（render 時に呼ぶ）
    pub fn set_display_width(&mut self, width: usize) {
        self.display_width = width;
    }

    /// スクロール位置以降の行を文字単位で事前ラップして返す。
    /// cursor_visual_position と同じ文字単位ラップなのでカーソル位置と描画が一致する。
    /// Paragraph には `.wrap()` を付けずに渡すこと。
    pub fn char_wrapped_lines_from_scroll(&self) -> Vec<String> {
        let w = self.effective_width();
        let start = self.scroll_offset.min(self.lines.len());
        let mut result = Vec::new();
        for line in &self.lines[start..] {
            if line.is_empty() || w == 0 {
                result.push(String::new());
                continue;
            }
            let mut current = String::new();
            let mut col = 0;
            for ch in line.chars() {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                if col + cw > w && col > 0 {
                    result.push(std::mem::take(&mut current));
                    col = 0;
                }
                current.push(ch);
                col += cw;
            }
            result.push(current);
        }
        result
    }

    /// カーソル位置に複数行テキストを挿入
    pub fn insert_text(&mut self, text: &str) {
        for (i, chunk) in text.split('\n').enumerate() {
            if i > 0 {
                self.insert_newline();
            }
            for ch in chunk.chars() {
                self.insert_char(ch);
            }
        }
    }

    /// カーソル位置に文字を挿入
    pub fn insert_char(&mut self, ch: char) {
        let line = &mut self.lines[self.cursor_row];
        line.insert(self.cursor_col, ch);
        self.cursor_col += ch.len_utf8();
    }

    /// カーソル位置で行を分割（改行挿入）
    pub fn insert_newline(&mut self) {
        let tail = self.lines[self.cursor_row][self.cursor_col..].to_string();
        self.lines[self.cursor_row].truncate(self.cursor_col);
        self.cursor_row += 1;
        self.lines.insert(self.cursor_row, tail);
        self.cursor_col = 0;
    }

    /// カーソル前の文字を削除（行頭なら前の行と結合）
    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let line = &self.lines[self.cursor_row];
            // カーソル手前の文字境界を探す
            let prev_boundary = line[..self.cursor_col]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.lines[self.cursor_row].remove(prev_boundary);
            self.cursor_col = prev_boundary;
        } else if self.cursor_row > 0 {
            let removed = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].push_str(&removed);
        }
    }

    /// カーソル位置の文字を削除（行末なら次の行と結合）
    pub fn delete(&mut self) {
        let line = &self.lines[self.cursor_row];
        if self.cursor_col < line.len() {
            self.lines[self.cursor_row].remove(self.cursor_col);
        } else if self.cursor_row + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    /// カーソルを左に移動（行頭なら前の行末に移動）
    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            let line = &self.lines[self.cursor_row];
            let prev = line[..self.cursor_col]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.cursor_col = prev;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
        }
    }

    /// カーソルを右に移動（行末なら次の行頭に移動）
    pub fn move_right(&mut self) {
        let line = &self.lines[self.cursor_row];
        if self.cursor_col < line.len() {
            let ch = line[self.cursor_col..].chars().next().unwrap();
            self.cursor_col += ch.len_utf8();
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// カーソルを上に移動（wrap 時は視覚行単位）
    pub fn move_up(&mut self) {
        self.move_up_visual();
    }

    /// カーソルを下に移動（wrap 時は視覚行単位）
    pub fn move_down(&mut self) {
        self.move_down_visual();
    }

    /// カーソルを行頭に移動
    pub fn move_home(&mut self) {
        self.cursor_col = 0;
    }

    /// カーソルを行末に移動
    pub fn move_end(&mut self) {
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    /// カーソルから行末まで削除（行末なら次行を結合）
    pub fn kill_line(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
            self.lines[self.cursor_row].truncate(self.cursor_col);
        } else if self.cursor_row + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    /// 行頭からカーソルまで削除
    pub fn kill_to_start(&mut self) {
        if self.cursor_col > 0 {
            self.lines[self.cursor_row] =
                self.lines[self.cursor_row][self.cursor_col..].to_string();
            self.cursor_col = 0;
        } else if self.cursor_row > 0 {
            let current = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].push_str(&current);
        }
    }

    /// scroll_offset を自動調整してカーソルが表示範囲内に収まるようにする（wrap 考慮）
    pub fn ensure_visible(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        // カーソルがスクロール位置より上にある場合
        if self.cursor_row < self.scroll_offset {
            self.scroll_offset = self.cursor_row;
        }
        // カーソルの視覚位置が表示範囲を超えている場合、スクロールを進める
        while self.visual_rows_to_cursor() >= visible_height && self.scroll_offset < self.cursor_row
        {
            self.scroll_offset += 1;
        }
    }

    /// wrap 考慮のカーソル表示位置（scroll_offset からの相対）
    /// Returns (visual_col, visual_row)
    pub fn cursor_visual_position(&self) -> (usize, usize) {
        self.cursor_visual_position_inner()
    }

    /// Scrollbar 用: (total_visual_rows, scroll_position) を返す
    pub fn scrollbar_state(&self, visible_height: usize) -> Option<(usize, usize)> {
        let total = self.total_visual_rows();
        if total <= visible_height {
            return None;
        }
        Some((total, self.scroll_visual_position()))
    }

    /// 一般的なエディタキー操作を処理。処理した場合 true を返す。
    /// モード固有のキー（Esc, Ctrl+S 等）は呼び出し側で先に処理すること。
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        // Emacs keybindings (Ctrl+*)
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('f') => self.move_right(),
                KeyCode::Char('b') => self.move_left(),
                KeyCode::Char('p') => self.move_up(),
                KeyCode::Char('n') => self.move_down(),
                KeyCode::Char('a') => self.move_home(),
                KeyCode::Char('e') => self.move_end(),
                KeyCode::Char('d') => self.delete(),
                KeyCode::Char('h') => self.backspace(),
                KeyCode::Char('k') => self.kill_line(),
                KeyCode::Char('u') => self.kill_to_start(),
                _ => return false,
            }
            return true;
        }
        match code {
            KeyCode::Enter => self.insert_newline(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Home => self.move_home(),
            KeyCode::End => self.move_end(),
            KeyCode::Char(c) => self.insert_char(c),
            _ => return false,
        }
        true
    }

    // --- private helpers ---

    /// wrap 計算に使う実効幅（0 の場合は wrap 無効として巨大値を返す）
    fn effective_width(&self) -> usize {
        if self.display_width == 0 {
            usize::MAX
        } else {
            self.display_width
        }
    }

    /// 指定行の表示行数（character-level wrap、ratatui の Wrap { trim: false } と同じ挙動）
    fn line_visual_height(&self, line_idx: usize, width: usize) -> usize {
        let line = &self.lines[line_idx];
        if line.is_empty() || width == 0 {
            return 1;
        }
        let mut rows = 1;
        let mut col = 0;
        for ch in line.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + cw > width {
                rows += 1;
                col = 0;
            }
            col += cw;
        }
        rows
    }

    /// scroll_offset からカーソルまでの視覚行数・列を計算（character-level wrap）
    fn visual_rows_to_cursor(&self) -> usize {
        let (_, row) = self.cursor_visual_position_inner();
        row
    }

    /// wrap 考慮のカーソル表示位置を内部計算
    fn cursor_visual_position_inner(&self) -> (usize, usize) {
        let w = self.effective_width();
        let mut visual_row = 0;
        for i in self.scroll_offset..self.cursor_row {
            visual_row += self.line_visual_height(i, w);
        }
        // カーソル行内でカーソル位置までの wrap をシミュレーション
        let line = &self.lines[self.cursor_row];
        let mut col = 0;
        let mut byte_pos = 0;
        while byte_pos < self.cursor_col {
            if let Some(ch) = line[byte_pos..].chars().next() {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                if col + cw > w {
                    visual_row += 1;
                    col = 0;
                }
                col += cw;
                byte_pos += ch.len_utf8();
            } else {
                break;
            }
        }
        // カーソルが表示幅の端に達した場合、次の visual row の先頭に折り返す
        if w > 0 && col >= w {
            visual_row += 1;
            col = 0;
        }
        (col, visual_row)
    }

    /// 全行の合計 visual rows（Scrollbar 用）
    fn total_visual_rows(&self) -> usize {
        let w = self.effective_width();
        let mut rows = 0;
        for i in 0..self.lines.len() {
            rows += self.line_visual_height(i, w);
        }
        rows
    }

    /// scroll_offset までの visual rows（Scrollbar 位置用）
    fn scroll_visual_position(&self) -> usize {
        let w = self.effective_width();
        let mut rows = 0;
        for i in 0..self.scroll_offset {
            rows += self.line_visual_height(i, w);
        }
        rows
    }

    /// 指定論理行の各 wrap 行の開始バイトオフセットを返す。
    /// 例: 幅5 で "abcdefgh" → [0, 5] （wrap行0は byte 0 から、wrap行1は byte 5 から）
    fn wrap_line_byte_offsets(&self, line_idx: usize, width: usize) -> Vec<usize> {
        let line = &self.lines[line_idx];
        let mut offsets = vec![0usize];
        if line.is_empty() || width == 0 {
            return offsets;
        }
        let mut col = 0;
        let mut byte_pos = 0;
        for ch in line.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + cw > width && col > 0 {
                offsets.push(byte_pos);
                col = 0;
            }
            col += cw;
            byte_pos += ch.len_utf8();
        }
        offsets
    }

    /// 指定論理行・指定 wrap 行内で、目標の表示列に最も近いバイトオフセットを返す。
    fn byte_offset_at_visual_col(
        &self,
        line_idx: usize,
        wrap_line: usize,
        target_col: usize,
    ) -> usize {
        let w = self.effective_width();
        let offsets = self.wrap_line_byte_offsets(line_idx, w);
        let start_byte = offsets.get(wrap_line).copied().unwrap_or(0);
        let end_byte = offsets
            .get(wrap_line + 1)
            .copied()
            .unwrap_or(self.lines[line_idx].len());

        let line = &self.lines[line_idx];
        let mut col = 0;
        let mut byte_pos = start_byte;
        for ch in line[start_byte..end_byte].chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + cw > target_col {
                break;
            }
            col += cw;
            byte_pos += ch.len_utf8();
        }
        byte_pos
    }

    /// カーソルの現在の wrap 行インデックスと wrap 行内の表示列を返す。
    /// cursor_visual_position_inner と同じラップシミュレーションを使い一貫性を保つ。
    fn cursor_wrap_position(&self) -> (usize, usize) {
        let w = self.effective_width();
        let line = &self.lines[self.cursor_row];
        let mut wrap_line = 0;
        let mut col = 0;
        let mut byte_pos = 0;
        while byte_pos < self.cursor_col {
            if let Some(ch) = line[byte_pos..].chars().next() {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                if col + cw > w && col > 0 {
                    wrap_line += 1;
                    col = 0;
                }
                col += cw;
                byte_pos += ch.len_utf8();
            } else {
                break;
            }
        }
        // カーソルが表示幅の端に達した場合、次の wrap 行の先頭
        if w > 0 && w != usize::MAX && col >= w {
            wrap_line += 1;
            col = 0;
        }
        (wrap_line, col)
    }

    /// カーソルを視覚行単位で上に移動（wrap 考慮）
    fn move_up_visual(&mut self) {
        let w = self.effective_width();
        if w == 0 || w == usize::MAX {
            if self.cursor_row > 0 {
                self.cursor_row -= 1;
                self.clamp_cursor_col();
            }
            return;
        }

        let (wrap_line, vis_col) = self.cursor_wrap_position();

        if wrap_line > 0 {
            self.cursor_col =
                self.byte_offset_at_visual_col(self.cursor_row, wrap_line - 1, vis_col);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            let prev_offsets = self.wrap_line_byte_offsets(self.cursor_row, w);
            let last_wrap = prev_offsets.len() - 1;
            self.cursor_col = self.byte_offset_at_visual_col(self.cursor_row, last_wrap, vis_col);
        }
    }

    /// カーソルを視覚行単位で下に移動（wrap 考慮）
    fn move_down_visual(&mut self) {
        let w = self.effective_width();
        if w == 0 || w == usize::MAX {
            if self.cursor_row + 1 < self.lines.len() {
                self.cursor_row += 1;
                self.clamp_cursor_col();
            }
            return;
        }

        let offsets = self.wrap_line_byte_offsets(self.cursor_row, w);
        let (wrap_line, vis_col) = self.cursor_wrap_position();

        if wrap_line + 1 < offsets.len() {
            self.cursor_col =
                self.byte_offset_at_visual_col(self.cursor_row, wrap_line + 1, vis_col);
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.byte_offset_at_visual_col(self.cursor_row, 0, vis_col);
        }
    }

    /// cursor_col が現在の行のバイト長を超えないようにクランプ
    /// 行移動時にバイト境界に合わせる
    fn clamp_cursor_col(&mut self) {
        let line = &self.lines[self.cursor_row];
        if self.cursor_col > line.len() {
            self.cursor_col = line.len();
        } else {
            // バイト境界に合わせる: cursor_col が文字の途中にある場合、前の文字境界に戻す
            while self.cursor_col > 0 && !line.is_char_boundary(self.cursor_col) {
                self.cursor_col -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト専用ゲッター
    impl TextEditor {
        fn line_count(&self) -> usize {
            self.lines.len()
        }
        fn cursor_row(&self) -> usize {
            self.cursor_row
        }
        fn cursor_col(&self) -> usize {
            self.cursor_col
        }
        fn scroll_offset(&self) -> usize {
            self.scroll_offset
        }
        /// 表示列を `unicode_width` で計算
        fn cursor_display_col(&self) -> usize {
            let line = &self.lines[self.cursor_row];
            line[..self.cursor_col]
                .chars()
                .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
                .sum()
        }
    }

    #[test]
    fn test_new_editor() {
        let editor = TextEditor::new();
        assert_eq!(editor.line_count(), 1);
        assert!(editor.is_empty());
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_display_col(), 0);
    }

    #[test]
    fn test_insert_char() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        editor.insert_char('c');
        assert_eq!(editor.text(), "abc");
        assert!(!editor.is_empty());
    }

    #[test]
    fn test_insert_newline() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        editor.insert_newline();
        editor.insert_char('c');
        assert_eq!(editor.text(), "ab\nc");
        assert_eq!(editor.line_count(), 2);
    }

    #[test]
    fn test_insert_newline_mid_line() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        editor.insert_char('c');
        // カーソルを 'b' の後ろに移動
        editor.move_left();
        editor.insert_newline();
        assert_eq!(editor.text(), "ab\nc");
    }

    #[test]
    fn test_backspace_basic() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        editor.backspace();
        assert_eq!(editor.text(), "a");
    }

    #[test]
    fn test_backspace_empty() {
        let mut editor = TextEditor::new();
        // 空の状態で backspace しても panic しない
        editor.backspace();
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn test_backspace_at_line_start_joins_lines() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        // カーソルを2行目の先頭に移動
        editor.move_home();
        editor.backspace();
        assert_eq!(editor.text(), "ab");
        assert_eq!(editor.line_count(), 1);
        assert_eq!(editor.cursor_row(), 0);
    }

    #[test]
    fn test_delete_basic() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        editor.move_home();
        editor.delete();
        assert_eq!(editor.text(), "b");
    }

    #[test]
    fn test_delete_at_line_end_joins_lines() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        // 1行目の末尾に移動
        editor.move_up();
        editor.move_end();
        editor.delete();
        assert_eq!(editor.text(), "ab");
        assert_eq!(editor.line_count(), 1);
    }

    #[test]
    fn test_move_left_right() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        assert_eq!(editor.cursor_display_col(), 2);
        editor.move_left();
        assert_eq!(editor.cursor_display_col(), 1);
        editor.move_right();
        assert_eq!(editor.cursor_display_col(), 2);
    }

    #[test]
    fn test_move_left_across_lines() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        editor.move_home();
        // 2行目の先頭から左に移動 → 1行目の末尾
        editor.move_left();
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_display_col(), 1); // 'a' の後ろ
    }

    #[test]
    fn test_move_right_across_lines() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        // 1行目の末尾に移動
        editor.move_up();
        editor.move_end();
        editor.move_right();
        assert_eq!(editor.cursor_row(), 1);
        assert_eq!(editor.cursor_display_col(), 0);
    }

    #[test]
    fn test_move_up_down() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        editor.insert_newline();
        editor.insert_char('c');
        assert_eq!(editor.cursor_row(), 2);
        editor.move_up();
        assert_eq!(editor.cursor_row(), 1);
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
        // 上端で move_up しても 0
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
        editor.move_down();
        assert_eq!(editor.cursor_row(), 1);
    }

    #[test]
    fn test_move_up_clamps_col() {
        let mut editor = TextEditor::new();
        // 1行目: "ab"、2行目: "cdefg"
        editor.insert_char('a');
        editor.insert_char('b');
        editor.insert_newline();
        editor.insert_char('c');
        editor.insert_char('d');
        editor.insert_char('e');
        editor.insert_char('f');
        editor.insert_char('g');
        // カーソルは2行目末尾(col=5)
        assert_eq!(editor.cursor_display_col(), 5);
        editor.move_up();
        // 1行目は "ab"(len=2) なのでクランプされる
        assert_eq!(editor.cursor_display_col(), 2);
    }

    #[test]
    fn test_move_home_end() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        editor.insert_char('c');
        editor.move_home();
        assert_eq!(editor.cursor_display_col(), 0);
        editor.move_end();
        assert_eq!(editor.cursor_display_col(), 3);
    }

    #[test]
    fn test_multibyte_insert() {
        let mut editor = TextEditor::new();
        editor.insert_char('あ');
        editor.insert_char('い');
        assert_eq!(editor.text(), "あい");
        // 全角文字は幅2
        assert_eq!(editor.cursor_display_col(), 4);
    }

    #[test]
    fn test_multibyte_backspace() {
        let mut editor = TextEditor::new();
        editor.insert_char('あ');
        editor.insert_char('い');
        editor.backspace();
        assert_eq!(editor.text(), "あ");
        assert_eq!(editor.cursor_display_col(), 2);
    }

    #[test]
    fn test_multibyte_move_left_right() {
        let mut editor = TextEditor::new();
        editor.insert_char('あ');
        editor.insert_char('い');
        editor.move_left();
        assert_eq!(editor.cursor_display_col(), 2); // 'あ' の後ろ
        editor.move_left();
        assert_eq!(editor.cursor_display_col(), 0);
        editor.move_right();
        assert_eq!(editor.cursor_display_col(), 2);
    }

    #[test]
    fn test_multibyte_clamp_on_line_move() {
        let mut editor = TextEditor::new();
        // 1行目: "a" (1バイト)、2行目: "あ" (3バイト)
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('あ');
        // カーソルは2行目末尾 (col=3バイト, display=2)
        assert_eq!(editor.cursor_col(), 3);
        assert_eq!(editor.cursor_display_col(), 2);
        editor.move_up();
        // 1行目は "a" (len=1) にクランプ
        assert_eq!(editor.cursor_col(), 1);
        assert_eq!(editor.cursor_display_col(), 1);
        editor.move_down();
        // 2行目に戻ると col=1 だが、"あ" は3バイトなので
        // バイト境界に合わせて col=0 になる
        assert_eq!(editor.cursor_display_col(), 0);
    }

    #[test]
    fn test_clear() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        editor.clear();
        assert!(editor.is_empty());
        assert_eq!(editor.line_count(), 1);
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_display_col(), 0);
    }

    #[test]
    fn test_text_multiline() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        editor.insert_newline();
        editor.insert_char('c');
        assert_eq!(editor.text(), "a\nb\nc");
    }

    #[test]
    fn test_ensure_visible_scrolls_down() {
        let mut editor = TextEditor::new();
        for i in 0..10 {
            editor.insert_char(char::from(b'a' + i));
            editor.insert_newline();
        }
        // カーソルは11行目 (index=10)
        assert_eq!(editor.cursor_row(), 10);
        editor.ensure_visible(5);
        // scroll_offset は cursor_row - visible_height + 1 = 10 - 5 + 1 = 6
        assert_eq!(editor.scroll_offset(), 6);
    }

    #[test]
    fn test_ensure_visible_scrolls_up() {
        let mut editor = TextEditor::new();
        for i in 0..10 {
            editor.insert_char(char::from(b'a' + i));
            editor.insert_newline();
        }
        editor.ensure_visible(5);
        assert_eq!(editor.scroll_offset(), 6);
        // カーソルを先頭に移動
        editor.cursor_row = 0;
        editor.cursor_col = 0;
        editor.ensure_visible(5);
        assert_eq!(editor.scroll_offset(), 0);
    }

    #[test]
    fn test_visible_lines() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        editor.insert_newline();
        editor.insert_char('c');
        let lines = editor.char_wrapped_lines_from_scroll();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "a");
        assert_eq!(lines[1], "b");
        assert_eq!(lines[2], "c");
    }

    #[test]
    fn test_delete_at_end_of_last_line() {
        let mut editor = TextEditor::new();
        editor.insert_char('a');
        // 最終行の末尾で delete しても何も起きない
        editor.delete();
        assert_eq!(editor.text(), "a");
    }

    #[test]
    fn test_mixed_ascii_and_multibyte() {
        let mut editor = TextEditor::new();
        editor.insert_char('H');
        editor.insert_char('i');
        editor.insert_char('！'); // 全角感嘆符
        assert_eq!(editor.text(), "Hi！");
        // 'H'=1, 'i'=1, '！'=2 → display_col=4
        assert_eq!(editor.cursor_display_col(), 4);
        editor.backspace();
        assert_eq!(editor.text(), "Hi");
        assert_eq!(editor.cursor_display_col(), 2);
    }

    #[test]
    fn test_cursor_visual_position_no_wrap() {
        let mut editor = TextEditor::new();
        // display_width=0 (wrap 無効)
        editor.insert_char('a');
        editor.insert_char('b');
        let (col, row) = editor.cursor_visual_position();
        assert_eq!(col, 2);
        assert_eq!(row, 0);
    }

    #[test]
    fn test_cursor_visual_position_with_wrap() {
        let mut editor = TextEditor::new();
        editor.set_display_width(5);
        // 10文字の行 → display_width=5 で2行分
        for c in "abcdefghij".chars() {
            editor.insert_char(c);
        }
        // カーソルは col=10、wrap で visual_col=0, visual_row=2
        let (col, row) = editor.cursor_visual_position();
        assert_eq!(col, 0);
        assert_eq!(row, 2);
    }

    #[test]
    fn test_ensure_visible_with_wrap() {
        let mut editor = TextEditor::new();
        editor.set_display_width(5);
        // 1行に20文字 → visual 4行
        for c in "abcdefghijklmnopqrst".chars() {
            editor.insert_char(c);
        }
        editor.insert_newline();
        // 2行目にも3文字
        for c in "xyz".chars() {
            editor.insert_char(c);
        }
        // カーソルは2行目。1行目が visual 4行、2行目の cursor_display_col=3
        // total visual from scroll=0: 4 + 1 = 5 (2行目のカーソル行内offset=0)
        // visible_height=5 → ギリギリ収まる
        editor.ensure_visible(5);
        assert_eq!(editor.scroll_offset(), 0);

        // visible_height=3 だと収まらない → スクロール
        editor.ensure_visible(3);
        assert!(editor.scroll_offset() > 0);
    }

    #[test]
    fn test_scrollbar_state() {
        let mut editor = TextEditor::new();
        for i in 0..10 {
            editor.insert_char(char::from(b'a' + i));
            editor.insert_newline();
        }
        // 初期状態: scroll_offset=0, 11行 > visible 5行 → Some
        assert!(editor.scrollbar_state(5).is_some());
        let (total, pos) = editor.scrollbar_state(5).unwrap();
        assert_eq!(total, 11);
        assert_eq!(pos, 0);

        // スクロールした後
        editor.ensure_visible(5);
        let (total2, pos2) = editor.scrollbar_state(5).unwrap();
        assert_eq!(total2, 11);
        assert!(pos2 > 0);

        // 全行が収まる場合 → None
        let small_editor = TextEditor::new();
        assert!(small_editor.scrollbar_state(5).is_none());
    }

    #[test]
    fn test_insert_text_single_line() {
        let mut editor = TextEditor::new();
        editor.insert_text("hello");
        assert_eq!(editor.text(), "hello");
        assert_eq!(editor.line_count(), 1);
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 5);
    }

    #[test]
    fn test_insert_text_multi_line() {
        let mut editor = TextEditor::new();
        editor.insert_text("line1\nline2\nline3");
        assert_eq!(editor.text(), "line1\nline2\nline3");
        assert_eq!(editor.line_count(), 3);
        assert_eq!(editor.cursor_row(), 2);
        assert_eq!(editor.cursor_col(), 5);
    }

    #[test]
    fn test_insert_text_at_cursor() {
        let mut editor = TextEditor::new();
        editor.insert_text("ab");
        editor.move_left();
        editor.insert_text("X\nY");
        // "a" + "X\nY" + "b" → "aX\nYb"
        assert_eq!(editor.text(), "aX\nYb");
        assert_eq!(editor.cursor_row(), 1);
        assert_eq!(editor.cursor_col(), 1); // 'Y' の後
    }

    #[test]
    fn test_kill_line_middle() {
        let mut editor = TextEditor::new();
        editor.insert_text("hello world");
        // カーソルを "hello " の後に移動（5文字戻る）
        for _ in 0..5 {
            editor.move_left();
        }
        editor.kill_line();
        assert_eq!(editor.text(), "hello ");
    }

    #[test]
    fn test_kill_line_at_end_joins_next() {
        let mut editor = TextEditor::new();
        editor.insert_text("aaa\nbbb");
        editor.move_up();
        editor.move_end();
        // 行末で kill_line → 次行と結合
        editor.kill_line();
        assert_eq!(editor.text(), "aaabbb");
        assert_eq!(editor.line_count(), 1);
    }

    #[test]
    fn test_kill_to_start_middle() {
        let mut editor = TextEditor::new();
        editor.insert_text("hello world");
        // カーソルを "hello " の後に移動
        for _ in 0..5 {
            editor.move_left();
        }
        editor.kill_to_start();
        assert_eq!(editor.text(), "world");
        assert_eq!(editor.cursor_col(), 0);
    }

    #[test]
    fn test_kill_to_start_at_beginning_joins_prev() {
        let mut editor = TextEditor::new();
        editor.insert_text("aaa\nbbb");
        // カーソルは2行目末尾、行頭に移動
        editor.move_home();
        // 行頭で kill_to_start → 前行と結合
        editor.kill_to_start();
        assert_eq!(editor.text(), "aaabbb");
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 3); // "aaa" の後
    }

    #[test]
    fn test_emacs_keybindings() {
        let mut editor = TextEditor::new();
        editor.insert_text("abc\ndef");

        let ctrl = KeyModifiers::CONTROL;

        // Ctrl+A: 行頭
        editor.handle_key(KeyCode::Char('a'), ctrl);
        assert_eq!(editor.cursor_col(), 0);

        // Ctrl+E: 行末
        editor.handle_key(KeyCode::Char('e'), ctrl);
        assert_eq!(editor.cursor_col(), 3); // "def".len()

        // Ctrl+P: 上移動
        editor.handle_key(KeyCode::Char('p'), ctrl);
        assert_eq!(editor.cursor_row(), 0);

        // Ctrl+N: 下移動
        editor.handle_key(KeyCode::Char('n'), ctrl);
        assert_eq!(editor.cursor_row(), 1);

        // Ctrl+F: 右移動
        editor.move_home();
        editor.handle_key(KeyCode::Char('f'), ctrl);
        assert_eq!(editor.cursor_col(), 1);

        // Ctrl+B: 左移動
        editor.handle_key(KeyCode::Char('b'), ctrl);
        assert_eq!(editor.cursor_col(), 0);

        // Ctrl+D: delete
        editor.handle_key(KeyCode::Char('d'), ctrl);
        assert_eq!(editor.text(), "abc\nef");

        // Ctrl+H: backspace (行頭 → 前行と結合)
        editor.move_home();
        editor.handle_key(KeyCode::Char('h'), ctrl);
        assert_eq!(editor.text(), "abcef");
        assert_eq!(editor.line_count(), 1);
    }

    #[test]
    fn test_visual_move_down_within_wrapped_line() {
        let mut editor = TextEditor::new();
        editor.set_display_width(5);
        // "abcdefghij" → wrap行0: "abcde", wrap行1: "fghij"
        for c in "abcdefghij".chars() {
            editor.insert_char(c);
        }
        // カーソルを先頭に移動
        editor.move_home(); // 論理行先頭 = wrap行0先頭
        assert_eq!(editor.cursor_col(), 0);

        // 下に移動 → 同じ論理行の wrap行1 先頭
        editor.move_down();
        // wrap行1 の先頭は byte offset 5 ("f" の位置)
        assert_eq!(editor.cursor_col(), 5);
        assert_eq!(editor.cursor_row(), 0); // 同じ論理行

        // もう一度下 → 論理行の末尾を超えているので次の論理行はない → 動かない
        editor.move_down();
        assert_eq!(editor.cursor_col(), 5);
        assert_eq!(editor.cursor_row(), 0);
    }

    #[test]
    fn test_visual_move_up_within_wrapped_line() {
        let mut editor = TextEditor::new();
        editor.set_display_width(5);
        // "abcdefghij" → wrap行0: "abcde", wrap行1: "fghij"
        for c in "abcdefghij".chars() {
            editor.insert_char(c);
        }
        // カーソルは末尾 (byte 10)。表示列は幅5に達するので wrap行2 の col=0
        // 上に移動 → wrap行1 の col=0 → byte 5
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 5);

        // もう一度上 → wrap行0 の col=0 → byte 0
        editor.move_up();
        assert_eq!(editor.cursor_col(), 0);
    }

    #[test]
    fn test_visual_move_across_logical_lines() {
        let mut editor = TextEditor::new();
        editor.set_display_width(5);
        // 行0: "abcdefghij" (2 wrap行), 行1: "xyz"
        for c in "abcdefghij".chars() {
            editor.insert_char(c);
        }
        editor.insert_newline();
        for c in "xyz".chars() {
            editor.insert_char(c);
        }
        // カーソルは行1 末尾 (col=3)
        assert_eq!(editor.cursor_row(), 1);

        // 上に移動 → 行0 の最後の wrap行 (wrap行1: "fghij") の col=3 → byte 8 ("i" の後)
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 8); // "fgh" = 3文字 → byte 5+3=8

        // もう一度上 → 行0 の wrap行0 ("abcde") の col=3 → byte 3
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 3);
    }

    #[test]
    fn test_visual_move_down_across_logical_lines() {
        let mut editor = TextEditor::new();
        editor.set_display_width(5);
        // 行0: "abcdefghij" (2 wrap行), 行1: "xyz"
        for c in "abcdefghij".chars() {
            editor.insert_char(c);
        }
        editor.insert_newline();
        for c in "xyz".chars() {
            editor.insert_char(c);
        }
        // カーソルを行0 先頭に移動
        editor.cursor_row = 0;
        editor.cursor_col = 0;

        // 下 → wrap行1 ("fghij") の col=0 → byte 5
        editor.move_down();
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 5);

        // 下 → 行1 の wrap行0 ("xyz") の col=0 → byte 0
        editor.move_down();
        assert_eq!(editor.cursor_row(), 1);
        assert_eq!(editor.cursor_col(), 0);
    }

    #[test]
    fn test_visual_move_no_wrap() {
        // display_width=0 (wrap 無効) → 従来の論理行移動
        let mut editor = TextEditor::new();
        editor.insert_text("abc\ndef\nghi");
        editor.cursor_row = 0;
        editor.cursor_col = 0;

        editor.move_down();
        assert_eq!(editor.cursor_row(), 1);
        editor.move_down();
        assert_eq!(editor.cursor_row(), 2);
        editor.move_up();
        assert_eq!(editor.cursor_row(), 1);
    }

    #[test]
    fn test_visual_move_with_multibyte() {
        let mut editor = TextEditor::new();
        editor.set_display_width(6);
        // "あいうえお" → 各2幅 = 合計10、幅6で wrap → wrap行0: "あいう" (6幅), wrap行1: "えお" (4幅)
        for c in "あいうえお".chars() {
            editor.insert_char(c);
        }
        // カーソルは末尾 = wrap行1
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
        // wrap行1 でのカーソル表示列は 4 ("えお"の後)、wrap行0 で col=4 → "あい" (各3byte) = 6byte
        // 表示列4 → "あ"(2) + "い"(2) = 4 → byte 6
        assert_eq!(editor.cursor_col(), 6);
    }
}
