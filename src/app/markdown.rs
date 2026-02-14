use super::ThemeMode;
use super::style::PrDescStyleSheet;
use ratatui::text::{Line, Span};

/// マークダウンテキストを ratatui Line に変換する。
/// テーブルブロックは tui_markdown を迂回してプレーンテキストとして保持する。
pub(super) fn render_markdown(text: &str, theme: ThemeMode) -> Vec<Line<'static>> {
    let blocks = split_blocks(text);
    let options = tui_markdown::Options::new(PrDescStyleSheet { theme });
    let mut lines = Vec::new();

    for block in blocks {
        match block {
            MdBlock::Text(t) => {
                let rendered = tui_markdown::from_str_with_options(&t, &options);
                for line in rendered.lines {
                    let mut new_line = Line::from(
                        line.spans
                            .into_iter()
                            .map(|span| Span::styled(span.content.into_owned(), span.style))
                            .collect::<Vec<_>>(),
                    );
                    new_line.style = line.style;
                    new_line.alignment = line.alignment;
                    lines.push(new_line);
                }
            }
            MdBlock::Table(rows) => {
                // テーブルはプレーンテキストとしてそのまま表示
                // (tui_markdown が改行を除去してしまうのを防ぐ)
                for row in &rows {
                    lines.push(Line::raw(row.to_string()));
                }
            }
        }
    }

    lines
}

// ── ブロック分割 ──────────────────────────────────

enum MdBlock {
    Text(String),
    Table(Vec<String>),
}

/// テキストをテーブルブロックと非テーブルブロックに分割する
fn split_blocks(text: &str) -> Vec<MdBlock> {
    let src_lines: Vec<&str> = text.lines().collect();
    let mut blocks: Vec<MdBlock> = Vec::new();
    let mut text_buf = String::new();
    let mut i = 0;

    while i < src_lines.len() {
        if is_table_line(src_lines[i]) {
            // テーブル候補: 連続する table line を集めて separator の有無を確認
            let start = i;
            let mut has_separator = false;
            while i < src_lines.len() && is_table_line(src_lines[i]) {
                if is_separator_line(src_lines[i]) {
                    has_separator = true;
                }
                i += 1;
            }

            if has_separator && i - start >= 2 {
                // 有効なテーブルブロック
                if !text_buf.is_empty() {
                    blocks.push(MdBlock::Text(std::mem::take(&mut text_buf)));
                }
                let table_lines: Vec<String> =
                    src_lines[start..i].iter().map(|l| l.to_string()).collect();
                blocks.push(MdBlock::Table(table_lines));
                continue;
            }

            // separator がない場合はテキストとして戻す
            for line in &src_lines[start..i] {
                if !text_buf.is_empty() {
                    text_buf.push('\n');
                }
                text_buf.push_str(line);
            }
            continue;
        }

        if !text_buf.is_empty() {
            text_buf.push('\n');
        }
        text_buf.push_str(src_lines[i]);
        i += 1;
    }

    if !text_buf.is_empty() {
        blocks.push(MdBlock::Text(text_buf));
    }

    blocks
}

fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.len() > 1
}

fn is_separator_line(line: &str) -> bool {
    let inner = line.trim().trim_matches('|');
    !inner.is_empty()
        && inner
            .chars()
            .all(|c| c == '-' || c == ':' || c == ' ' || c == '|')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_table_line() {
        assert!(is_table_line("| a | b |"));
        assert!(is_table_line("| --- | --- |"));
        assert!(!is_table_line("not a table"));
        assert!(!is_table_line("|")); // too short
        assert!(!is_table_line(""));
    }

    #[test]
    fn test_is_separator_line() {
        assert!(is_separator_line("| --- | --- |"));
        assert!(is_separator_line("| :--- | ---: |"));
        assert!(is_separator_line("|---|---|"));
        assert!(!is_separator_line("| a | b |"));
    }

    #[test]
    fn test_split_blocks_no_table() {
        let text = "# Hello\n\nSome text";
        let blocks = split_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], MdBlock::Text(t) if t == text));
    }

    #[test]
    fn test_split_blocks_table_only() {
        let text = "| a | b |\n| - | - |\n| 1 | 2 |";
        let blocks = split_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], MdBlock::Table(rows) if rows.len() == 3));
    }

    #[test]
    fn test_split_blocks_text_then_table() {
        let text = "Some text\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nMore text";
        let blocks = split_blocks(text);
        assert_eq!(blocks.len(), 3);
        assert!(matches!(&blocks[0], MdBlock::Text(_)));
        assert!(matches!(&blocks[1], MdBlock::Table(rows) if rows.len() == 3));
        assert!(matches!(&blocks[2], MdBlock::Text(t) if t.contains("More text")));
    }

    #[test]
    fn test_split_blocks_pipe_without_separator_is_text() {
        let text = "| just a pipe line |";
        let blocks = split_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], MdBlock::Text(_)));
    }

    #[test]
    fn test_render_markdown_preserves_table_lines() {
        let text = "# Title\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nEnd";
        let lines = render_markdown(text, ThemeMode::Dark);
        assert!(!lines.is_empty());
        // テーブル行がプレーンテキストとして保持されていることを確認
        let text_content: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text_content.contains("| a | b |"));
        assert!(text_content.contains("| 1 | 2 |"));
    }
}
