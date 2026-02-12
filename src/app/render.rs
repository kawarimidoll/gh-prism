use super::*;

use crate::git::diff::highlight_diff;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, HorizontalAlignment, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use ratatui_image::StatefulImage;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

impl App {
    pub(super) fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        // CommentInput モードでは入力欄を下部に表示
        let main_layout = if self.mode == AppMode::CommentInput {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Min(0),
                    Constraint::Length(3),
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
            AppMode::CommentInput => " [COMMENT] ",
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
            AppMode::CommentInput => Color::Green,
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
        let left_full = format!(" prism - {}#{} | ?: help", self.repo, self.pr_number,);
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
            self.pr_desc_rect = Rect::default();
            self.commit_list_rect = Rect::default();
            self.file_tree_rect = Rect::default();
            self.diff_view_rect = Rect::default();

            match self.focused_panel {
                Panel::PrDescription => {
                    self.pr_desc_rect = full_area;
                    self.render_pr_description(frame, full_area);
                }
                Panel::CommitList => {
                    self.commit_list_rect = full_area;
                    self.render_commit_list_stateful(frame, full_area);
                }
                Panel::FileTree => {
                    self.file_tree_rect = full_area;
                    self.render_file_tree(frame, full_area);
                }
                Panel::DiffView => {
                    self.diff_view_rect = full_area;
                    self.render_diff_view_widget(frame, full_area);
                }
            }
        } else {
            // 通常表示: サイドバー30% + Diff70%
            let body_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(main_layout[1]);

            let sidebar_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(30),
                    Constraint::Percentage(35),
                    Constraint::Percentage(35),
                ])
                .split(body_layout[0]);

            let diff_area = body_layout[1];

            // マウスヒットテスト用に各ペインの Rect を記録
            self.pr_desc_rect = sidebar_layout[0];
            self.commit_list_rect = sidebar_layout[1];
            self.file_tree_rect = sidebar_layout[2];
            self.diff_view_rect = diff_area;

            // サイドバー3ペイン描画
            self.render_pr_description(frame, sidebar_layout[0]);
            self.render_commit_list_stateful(frame, sidebar_layout[1]);
            self.render_file_tree(frame, sidebar_layout[2]);
            // diff_view_height は render_diff_view_widget 内で正確に更新
            self.render_diff_view_widget(frame, diff_area);
        }

        // CommentInput モードでは入力欄を描画
        if self.mode == AppMode::CommentInput {
            self.render_comment_input(frame, main_layout[2]);
        }

        // ダイアログ描画（画面中央にオーバーレイ）
        match self.mode {
            AppMode::CommentView => self.render_comment_view_dialog(frame, area),
            AppMode::ReviewSubmit => self.render_review_submit_dialog(frame, area),
            AppMode::ReviewBodyInput => self.render_review_body_input_dialog(frame, area),
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
        let paragraph = Paragraph::new(text)
            .block(
                Block::default()
                    .title(" PR Description ")
                    .borders(Borders::ALL)
                    .border_style(style),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.pr_desc_scroll, 0));

        // Wrap 考慮済み視覚行数を計算（スクロール上限に使用）
        self.pr_desc_visual_total = paragraph.line_count(inner_width) as u16;
        // zoom 切替等で描画幅が変わった場合にスクロール位置をクランプ
        self.clamp_pr_desc_scroll();

        frame.render_widget(paragraph, area);
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
        let title = format!(" Commits ({}/{}) ", viewed_count, self.commits.len());
        let list = List::new(items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(style),
            )
            .highlight_style(self.highlight_style());

        frame.render_stateful_widget(list, area, &mut self.commit_list_state);
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
            .filter(|f| self.viewed_files.contains(&f.filename))
            .count();
        let items: Vec<ListItem> = files
            .iter()
            .map(|f| {
                let is_viewed = self.viewed_files.contains(&f.filename);
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
                let left_part = format!("{}{} {}", marker, status, f.filename);
                let mut spans = vec![
                    Span::styled(marker, text_style),
                    Span::styled(format!("{}", status), Style::default().fg(status_color)),
                    Span::styled(format!(" {}", f.filename), text_style),
                ];
                if comment_count > 0 {
                    let badge = format!("💬 {} ", comment_count);
                    // ボーダー左右 (2) を除いた内部幅
                    let inner = area.width.saturating_sub(2) as usize;
                    let left_width = UnicodeWidthStr::width(left_part.as_str());
                    let badge_width = UnicodeWidthStr::width(badge.as_str());
                    let pad = inner.saturating_sub(left_width + badge_width);
                    spans.push(Span::styled(" ".repeat(pad), text_style));
                    spans.push(Span::styled(badge, Style::default().fg(Color::Yellow)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let title = format!(" Files ({}/{}) ", viewed_count, files.len());
        let list = List::new(items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(style),
            )
            .highlight_style(self.highlight_style());

        frame.render_stateful_widget(list, area, &mut self.file_list_state);
    }

    fn render_diff_view_widget(&mut self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focused_panel == Panel::DiffView {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        // コミットメッセージを取得
        let commit_msg = self
            .commit_list_state
            .selected()
            .and_then(|idx| self.commits.get(idx))
            .map(|c| c.commit.message.as_str())
            .unwrap_or("");

        let msg_line_count = if commit_msg.is_empty() {
            0u16
        } else {
            commit_msg.lines().count() as u16
        };

        // コミットメッセージがあればエリアを上下分割
        // メッセージ領域: 行数 + 2（ボーダー上下）、最大で area の 1/3
        let (msg_area, diff_area) = if msg_line_count > 0 {
            let msg_height = (msg_line_count + 2).min(area.height / 3).max(3);
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(msg_height), Constraint::Min(3)])
                .split(area);
            (Some(layout[0]), layout[1])
        } else {
            (None, area)
        };

        // DiffView の表示可能サイズを更新（ボーダー分を引く）
        self.diff_view_height = diff_area.height.saturating_sub(2);
        self.diff_view_width = diff_area.width.saturating_sub(2);

        // コミットメッセージ描画
        if let Some(msg_area) = msg_area {
            let msg_paragraph = Paragraph::new(commit_msg)
                .block(
                    Block::default()
                        .title(" Commit ")
                        .borders(Borders::ALL)
                        .border_style(border_style),
                )
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(msg_paragraph, msg_area);
        }

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
                    let count = sel.count(self.cursor_line);
                    format!(
                        " - {} line{} selected",
                        count,
                        if count == 1 { "" } else { "s" }
                    )
                }
                _ => String::new(),
            };

            let file_path_part = if has_file && !filename.is_empty() {
                let wrap_width = if self.diff_wrap { 7 } else { 0 }; // " [WRAP]"
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

            let wrap_suffix = if self.diff_wrap { " [WRAP]" } else { "" };

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

        // バイナリファイルまたは diff がない場合
        if has_file && !has_patch {
            let paragraph = Paragraph::new(Line::styled(
                "Binary file or no diff available",
                Style::default().fg(Color::DarkGray),
            ))
            .block(block);
            frame.render_widget(paragraph, diff_area);
            return;
        }

        // delta 出力をキャッシュ（ファイル選択が変わったときだけ再実行）
        let commit_idx = self.commit_list_state.selected().unwrap_or(usize::MAX);
        let file_idx = self.file_list_state.selected().unwrap_or(usize::MAX);
        let inner_width = diff_area.width.saturating_sub(2);

        let cache_hit = matches!(
            &self.diff_highlight_cache,
            Some((ci, fi, _)) if *ci == commit_idx && *fi == file_idx
        );

        if !cache_hit {
            let is_whole_file = matches!(file_status.as_str(), "added" | "removed" | "deleted");
            let base_text =
                if let Some(highlighted) = highlight_diff(&patch, &filename, &file_status) {
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
            self.diff_highlight_cache = Some((commit_idx, file_idx, base_text));
        }

        // キャッシュからクローンしてオーバーレイ適用用の可変テキストを作成
        let mut text = self.diff_highlight_cache.as_ref().unwrap().2.clone();

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
        if self.diff_wrap {
            for line in &mut text.lines {
                if line.spans.iter().all(|s| s.content.trim().is_empty()) {
                    line.spans.clear();
                }
            }
        }

        // 行番号プレフィックスを各行の先頭に挿入
        if self.show_line_numbers {
            use crate::github::review::parse_hunk_header;

            let line_num_style = Style::default().fg(Color::DarkGray);
            let separator_style = Style::default().fg(Color::DarkGray);
            let mut old_line: usize = 0;
            let mut new_line: usize = 0;

            // 追加/削除ファイルは片側の行番号のみ表示
            let show_old = !matches!(file_status.as_str(), "added");
            let show_new = !matches!(file_status.as_str(), "removed" | "deleted");

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
                                "     ".to_string()
                            } else {
                                let s = format!("{:>4} ", old_line);
                                old_line += 1;
                                s
                            };
                            prefix.push(Span::styled(old_str, line_num_style));
                        }

                        if show_new {
                            let new_str = if raw.starts_with('-') {
                                "     ".to_string()
                            } else {
                                let s = format!("{:>4} ", new_line);
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

        // 既存コメントの下線 / 💬 マーカーをテキスト側に適用
        // 背景色オーバーレイ（カーソル/選択/pending）は render 後に Buffer で全幅適用する
        let show_cursor = self.focused_panel == Panel::DiffView;
        let has_selection = self.mode == AppMode::LineSelect || self.mode == AppMode::CommentInput;
        let existing_counts = self.existing_comment_counts();
        let cursor_bg = match self.theme {
            ThemeMode::Dark => Color::DarkGray,
            ThemeMode::Light => Color::Indexed(254),
        };
        let pending_bg = match self.theme {
            ThemeMode::Dark => Color::Indexed(22),
            ThemeMode::Light => Color::Indexed(151),
        };

        // 背景色が必要な論理行を収集（render 後に Buffer で適用）
        let mut bg_lines: Vec<(usize, Color)> = Vec::new();

        for (idx, line) in text.lines.iter_mut().enumerate() {
            let is_selected = has_selection
                && self.line_selection.is_some_and(|sel| {
                    let (start, end) = sel.range(self.cursor_line);
                    idx >= start && idx <= end
                });
            let is_cursor = show_cursor && !has_selection && idx == self.cursor_line;
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

        // Wrap 有効時、レンダリングに使う実テキストから視覚行オフセットを計算してキャッシュ。
        // visual_line_offset / visual_to_logical_line はこのキャッシュを参照する。
        if self.diff_wrap {
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
            self.diff_visual_offsets = Some(offsets);
        } else {
            self.diff_visual_offsets = None;
        }

        let paragraph = Paragraph::new(text)
            .block(block)
            .scroll((self.diff_scroll, 0));
        let paragraph = if self.diff_wrap {
            paragraph.wrap(Wrap { trim: false })
        } else {
            paragraph
        };
        frame.render_widget(paragraph, diff_area);

        // Buffer に直接背景色を適用（全幅ハイライト）
        // Paragraph render 後に適用することで空行や行末の余白もカバーする
        if !bg_lines.is_empty() {
            let inner = Rect {
                x: diff_area.x + 1,
                y: diff_area.y + 1,
                width: inner_width,
                height: diff_area.height.saturating_sub(2),
            };
            let scroll = self.diff_scroll as usize;
            let buf = frame.buffer_mut();
            for &(logical_line, bg_color) in &bg_lines {
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
    }

    fn render_comment_input(&self, frame: &mut Frame, area: Rect) {
        let selection_info = if let Some(selection) = self.line_selection {
            let (start, end) = selection.range(self.cursor_line);
            format!(" L{}–L{} ", start + 1, end + 1)
        } else {
            String::new()
        };

        let block = Block::default()
            .title(format!(" Comment{} ", selection_info))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green));

        let paragraph = Paragraph::new(self.review.comment_input.as_str()).block(block);
        frame.render_widget(paragraph, area);

        // set_cursor_position でリアルカーソルを表示（表示幅で計算）
        frame.set_cursor_position(Position::new(
            area.x + self.review.comment_input.width() as u16 + 1, // +1 for border
            area.y + 1,                                            // +1 for border
        ));
    }

    /// 中央に固定サイズの矩形を配置
    fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        Rect::new(x, y, width.min(area.width), height.min(area.height))
    }

    fn render_review_submit_dialog(&self, frame: &mut Frame, area: Rect) {
        let dialog = Self::centered_rect(36, 10, area);
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
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "  j/k: select  Enter: next",
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::styled(
            "  Esc: cancel",
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

    fn render_review_body_input_dialog(&self, frame: &mut Frame, area: Rect) {
        let dialog = Self::centered_rect(50, 8, area);
        frame.render_widget(Clear, dialog);

        let event = self.available_events()[self.review.review_event_cursor];

        // ダイアログ内で表示できる入力テキスト幅を計算
        // dialog 内部幅 = dialog.width - 2(border), プレフィックス "  > " = 4文字
        let max_visible = dialog.width.saturating_sub(2 + 4) as usize;
        let input_width = self.review.review_body_input.width();
        let visible_text = if input_width <= max_visible {
            self.review.review_body_input.as_str()
        } else {
            // 末尾を表示: バイト境界を正しく扱うため文字単位でスキップ
            let skip_width = input_width - max_visible;
            let mut w = 0;
            let mut byte_offset = 0;
            for (i, ch) in self.review.review_body_input.char_indices() {
                if w >= skip_width {
                    byte_offset = i;
                    break;
                }
                w += ch.width().unwrap_or(0);
                byte_offset = i + ch.len_utf8();
            }
            &self.review.review_body_input[byte_offset..]
        };

        let lines = vec![
            Line::raw(""),
            Line::styled(
                format!("  Event: {}", event.label()),
                Style::default().fg(Color::Cyan),
            ),
            Line::raw(""),
            Line::styled(format!("  > {}", visible_text), Style::default()),
            Line::raw(""),
            Line::styled(
                "  Enter: submit  Esc: back",
                Style::default().fg(Color::DarkGray),
            ),
        ];

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .title(" Review Body (optional) ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        );
        frame.render_widget(paragraph, dialog);

        // カーソル表示（表示テキストの末尾に配置）
        let cursor_x = dialog.x + 5 + visible_text.width() as u16;
        frame.set_cursor_position((cursor_x, dialog.y + 4));
    }

    fn render_quit_confirm_dialog(&self, frame: &mut Frame, area: Rect) {
        let dialog = Self::centered_rect(38, 9, area);
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

    fn render_comment_view_dialog(&mut self, frame: &mut Frame, area: Rect) {
        // ダイアログサイズ: 幅60, 高さはコメント数に応じて動的（最大 area の 2/3）
        let content_height: u16 = self
            .review
            .viewing_comments
            .iter()
            .map(|c| {
                // @user (date) + 本文行数 + 空行
                1 + c.body.lines().count() as u16 + 1
            })
            .sum::<u16>()
            .max(3);
        let dialog_height = (content_height + 4).min(area.height * 2 / 3); // +4 for borders + footer
        let dialog_width = 60.min(area.width.saturating_sub(4));
        let dialog = Self::centered_rect(dialog_width, dialog_height, area);
        frame.render_widget(Clear, dialog);

        let mut lines = vec![Line::raw("")];
        for comment in &self.review.viewing_comments {
            lines.push(Line::styled(
                format!(
                    "  @{} ({})",
                    comment.user.login,
                    &comment.created_at[..10.min(comment.created_at.len())]
                ),
                Style::default().fg(Color::Cyan),
            ));
            for body_line in comment.body.lines() {
                lines.push(Line::raw(format!("  {}", body_line)));
            }
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            "  Esc/Enter/q: close",
            Style::default().fg(Color::DarkGray),
        ));

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Review Comments ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: false });

        // Paragraph::line_count() で wrap 考慮の正確な視覚行数を取得
        let visual_total = paragraph.line_count(dialog_width) as u16;
        let visible_height = dialog_height.saturating_sub(2);
        self.review.comment_view_max_scroll = visual_total.saturating_sub(visible_height);

        let paragraph = paragraph.scroll((self.review.viewing_comment_scroll, 0));
        frame.render_widget(paragraph, dialog);
    }

    fn render_help_dialog(&self, frame: &mut Frame, area: Rect) {
        let dialog_height = (area.height * 2 / 3)
            .max(20)
            .min(area.height.saturating_sub(4));
        let dialog_width = 50.min(area.width.saturating_sub(4));
        let dialog = Self::centered_rect(dialog_width, dialog_height, area);
        frame.render_widget(Clear, dialog);

        let s = Style::default().fg(Color::Yellow); // section
        let k = Style::default().fg(Color::Cyan); // key
        let d = Style::default(); // desc

        // (key, desc) のペアか、セクションヘッダーを表す enum 的タプル配列
        let entries: Vec<(&str, &str)> = vec![
            ("", "Navigation"),
            ("j / ↓", "Move down"),
            ("k / ↑", "Move up"),
            ("l / → / Tab", "Next pane"),
            ("h / ← / BackTab", "Previous pane"),
            ("1 / 2 / 3", "Jump to pane"),
            ("Enter", "Open diff / comment / media"),
            ("Esc", "Back to Files pane"),
            ("", "Scroll (Desc / Diff)"),
            ("Ctrl+d / Ctrl+u", "Half page down / up"),
            ("Ctrl+f / Ctrl+b", "Full page down / up"),
            ("g / G", "Top / Bottom"),
            ("", "Diff Jump"),
            ("]c / [c", "Next / prev change block"),
            ("]h / [h", "Next / prev hunk"),
            ("", "Selection & Comment"),
            ("v", "Enter line select mode"),
            ("c", "Comment on current line"),
            ("S", "Submit review"),
            ("", "Copy"),
            ("y", "Copy SHA / file path"),
            ("Y", "Copy commit message"),
            ("", "Other"),
            ("n", "Toggle line numbers (Diff)"),
            ("w", "Toggle line wrap (Diff)"),
            ("z", "Toggle zoom"),
            ("x", "Toggle viewed (Files/Commits)"),
            ("?", "This help"),
            ("q", "Quit"),
        ];

        let mut lines: Vec<Line> = vec![];
        for (key, desc) in &entries {
            if key.is_empty() {
                // セクションヘッダー
                lines.push(Line::raw(""));
                lines.push(Line::styled(format!("  {desc}"), s));
                lines.push(Line::styled("  ──────────────────────────", s));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {key:<18}"), k),
                    Span::styled(*desc, d),
                ]));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "  ?/Esc/q: close",
            Style::default().fg(Color::DarkGray),
        ));

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Help ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .scroll((self.help_scroll, 0));
        frame.render_widget(paragraph, dialog);
    }

    /// メディアビューアオーバーレイを描画する
    fn render_media_viewer_overlay(&mut self, frame: &mut Frame, area: Rect) {
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
        } else if let Some(ref mut protocol) = self.media_viewer_protocol {
            let widget = StatefulImage::default();
            frame.render_stateful_widget(widget, content_area, protocol);
        } else {
            let msg = Paragraph::new("Press o to open in browser")
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: false });
            let centered = Self::centered_rect(30, 1, content_area);
            frame.render_widget(msg, centered);
        }
    }
}
