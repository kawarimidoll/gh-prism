use crate::github::commits::CommitInfo;
use crate::github::files::DiffFile;
use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Panel {
    CommitList,
    FileTree,
    DiffView,
}

pub struct App {
    should_quit: bool,
    focused_panel: Panel,
    pr_number: u64,
    repo: String,
    pr_title: String,
    commits: Vec<CommitInfo>,
    commit_list_state: ListState,
    files_map: HashMap<String, Vec<DiffFile>>,
    file_list_state: ListState,
    diff_scroll: u16,
}

impl App {
    pub fn new(
        pr_number: u64,
        repo: String,
        pr_title: String,
        commits: Vec<CommitInfo>,
        files_map: HashMap<String, Vec<DiffFile>>,
    ) -> Self {
        let mut commit_list_state = ListState::default();
        if !commits.is_empty() {
            commit_list_state.select(Some(0));
        }

        // 最初のコミットのファイル数に基づいて file_list_state を初期化
        let mut file_list_state = ListState::default();
        if let Some(first_commit) = commits.first() {
            if let Some(files) = files_map.get(&first_commit.sha) {
                if !files.is_empty() {
                    file_list_state.select(Some(0));
                }
            }
        }

        Self {
            should_quit: false,
            focused_panel: Panel::CommitList,
            pr_number,
            repo,
            pr_title,
            commits,
            commit_list_state,
            files_map,
            file_list_state,
            diff_scroll: 0,
        }
    }

    /// 現在選択中のコミットのファイル一覧を取得
    fn current_files(&self) -> &[DiffFile] {
        if let Some(idx) = self.commit_list_state.selected() {
            if let Some(commit) = self.commits.get(idx) {
                if let Some(files) = self.files_map.get(&commit.sha) {
                    return files;
                }
            }
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
        self.diff_scroll = 0;
    }

    /// 現在選択中のファイルの patch を取得
    fn current_patch(&self) -> Option<&str> {
        let files = self.current_files();
        if let Some(idx) = self.file_list_state.selected() {
            if let Some(file) = files.get(idx) {
                return file.patch.as_deref();
            }
        }
        None
    }

    pub fn run(&mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let header_text = format!(
            " prism - {} PR #{}: {} | Tab: switch | j/k: select | Ctrl+d/u: scroll | g/G: top/end | q: quit",
            self.repo, self.pr_number, self.pr_title
        );

        frame.render_widget(
            Paragraph::new(header_text).style(Style::default().bg(Color::Blue).fg(Color::White)),
            main_layout[0],
        );

        let body_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(main_layout[1]);

        let sidebar_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(body_layout[0]);

        // コミットリストをStatefulWidgetとして描画
        self.render_commit_list_stateful(frame, sidebar_layout[0]);
        self.render_file_tree(frame, sidebar_layout[1]);
        self.render_diff_view_widget(frame, body_layout[1]);
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
            .map(|c| ListItem::new(format!("{} {}", c.short_sha(), c.message_summary())))
            .collect();

        let title = format!(" Commits ({}) ", self.commits.len());
        let list = List::new(items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(style),
            )
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));

        frame.render_stateful_widget(list, area, &mut self.commit_list_state);
    }

    fn render_file_tree(&mut self, frame: &mut Frame, area: Rect) {
        let style = if self.focused_panel == Panel::FileTree {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let files = self.current_files();
        let items: Vec<ListItem> = files
            .iter()
            .map(|f| {
                let display = format!("{} {} {}", f.status_char(), f.filename, f.changes_display());
                ListItem::new(display)
            })
            .collect();

        let title = format!(" Files ({}) ", files.len());
        let list = List::new(items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(style),
            )
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));

        frame.render_stateful_widget(list, area, &mut self.file_list_state);
    }

    fn render_diff_view_widget(&self, frame: &mut Frame, area: Rect) {
        let style = if self.focused_panel == Panel::DiffView {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let block = Block::default()
            .title(" Diff ")
            .borders(Borders::ALL)
            .border_style(style);

        // 選択中ファイルの patch を取得
        let content = self.current_patch().unwrap_or("");

        // 各行を色分けして Line に変換
        let lines: Vec<Line> = content
            .lines()
            .map(|line| {
                let style = match line.chars().next() {
                    Some('+') => Style::default().fg(Color::Green),
                    Some('-') => Style::default().fg(Color::Red),
                    Some('@') => Style::default().fg(Color::Cyan),
                    _ => Style::default(),
                };
                Line::styled(line, style)
            })
            .collect();

        let paragraph = Paragraph::new(lines)
            .block(block)
            .scroll((self.diff_scroll, 0));
        frame.render_widget(paragraph, area);
    }

    fn handle_events(&mut self) -> Result<()> {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => self.should_quit = true,
                    KeyCode::Tab => self.next_panel(),
                    KeyCode::BackTab => self.prev_panel(),
                    KeyCode::Char('j') => self.select_next(),
                    KeyCode::Char('k') => self.select_prev(),
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_diff_down();
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_diff_up();
                    }
                    KeyCode::Char('g') => {
                        if self.focused_panel == Panel::DiffView {
                            self.diff_scroll = 0;
                        }
                    }
                    KeyCode::Char('G') => {
                        if self.focused_panel == Panel::DiffView {
                            self.scroll_diff_to_end();
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn select_next(&mut self) {
        match self.focused_panel {
            Panel::CommitList if !self.commits.is_empty() => {
                let current = self.commit_list_state.selected().unwrap_or(0);
                let next = (current + 1) % self.commits.len();
                self.commit_list_state.select(Some(next));
                // ファイル選択をリセット
                self.reset_file_selection();
            }
            Panel::FileTree => {
                let files_len = self.current_files().len();
                if files_len > 0 {
                    let current = self.file_list_state.selected().unwrap_or(0);
                    let next = (current + 1) % files_len;
                    self.file_list_state.select(Some(next));
                    self.diff_scroll = 0;
                }
            }
            Panel::DiffView => {
                self.diff_scroll = self.diff_scroll.saturating_add(1);
            }
            _ => {}
        }
    }

    fn select_prev(&mut self) {
        match self.focused_panel {
            Panel::CommitList if !self.commits.is_empty() => {
                let current = self.commit_list_state.selected().unwrap_or(0);
                let prev = if current == 0 {
                    self.commits.len() - 1
                } else {
                    current - 1
                };
                self.commit_list_state.select(Some(prev));
                // ファイル選択をリセット
                self.reset_file_selection();
            }
            Panel::FileTree => {
                let files_len = self.current_files().len();
                if files_len > 0 {
                    let current = self.file_list_state.selected().unwrap_or(0);
                    let prev = if current == 0 {
                        files_len - 1
                    } else {
                        current - 1
                    };
                    self.file_list_state.select(Some(prev));
                    self.diff_scroll = 0;
                }
            }
            Panel::DiffView => {
                self.diff_scroll = self.diff_scroll.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn scroll_diff_down(&mut self) {
        if self.focused_panel == Panel::DiffView {
            // 半ページ分（10行）スクロール
            self.diff_scroll = self.diff_scroll.saturating_add(10);
        }
    }

    fn scroll_diff_up(&mut self) {
        if self.focused_panel == Panel::DiffView {
            self.diff_scroll = self.diff_scroll.saturating_sub(10);
        }
    }

    fn scroll_diff_to_end(&mut self) {
        if let Some(patch) = self.current_patch() {
            let line_count = patch.lines().count() as u16;
            // 画面に収まる分を引く（おおよそ10行）
            self.diff_scroll = line_count.saturating_sub(10);
        }
    }

    fn next_panel(&mut self) {
        self.focused_panel = match self.focused_panel {
            Panel::CommitList => Panel::FileTree,
            Panel::FileTree => Panel::DiffView,
            Panel::DiffView => Panel::CommitList,
        }
    }
    fn prev_panel(&mut self) {
        self.focused_panel = match self.focused_panel {
            Panel::CommitList => Panel::DiffView,
            Panel::FileTree => Panel::CommitList,
            Panel::DiffView => Panel::FileTree,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::commits::{CommitDetail, CommitInfo};

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
            vec![],
            create_empty_files_map(),
        );
        assert!(!app.should_quit);
        assert_eq!(app.focused_panel, Panel::CommitList);
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
            commits,
            create_empty_files_map(),
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
            commits,
            files_map,
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
            vec![],
            create_empty_files_map(),
        );
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::DiffView);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_prev_panel() {
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            vec![],
            create_empty_files_map(),
        );
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::DiffView);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_select_next_commits() {
        let commits = create_test_commits();
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            commits,
            create_empty_files_map(),
        );
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
            commits,
            create_empty_files_map(),
        );
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
            commits,
            files_map,
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
            commits,
            files_map,
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
            commits,
            files_map,
        );
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
            commits,
            create_empty_files_map(),
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
            commits,
            files_map,
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
            commits,
            files_map,
        );

        // ファイル一覧に移動して2番目のファイルを選択
        app.next_panel();
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
            commits,
            create_empty_files_map(),
        );
        assert_eq!(app.diff_scroll, 0);
    }

    #[test]
    fn test_scroll_diff_down() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            commits,
            files_map,
        );
        app.focused_panel = Panel::DiffView;
        assert_eq!(app.diff_scroll, 0);

        app.scroll_diff_down();
        assert_eq!(app.diff_scroll, 10);

        app.scroll_diff_down();
        assert_eq!(app.diff_scroll, 20);
    }

    #[test]
    fn test_scroll_diff_up() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            commits,
            files_map,
        );
        app.focused_panel = Panel::DiffView;
        app.diff_scroll = 20;

        app.scroll_diff_up();
        assert_eq!(app.diff_scroll, 10);

        app.scroll_diff_up();
        assert_eq!(app.diff_scroll, 0);

        // Should not go below 0
        app.scroll_diff_up();
        assert_eq!(app.diff_scroll, 0);
    }

    #[test]
    fn test_scroll_only_works_in_diff_panel() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            commits,
            files_map,
        );
        // CommitList panel (default)
        app.scroll_diff_down();
        assert_eq!(app.diff_scroll, 0);

        app.next_panel(); // FileTree
        app.scroll_diff_down();
        assert_eq!(app.diff_scroll, 0);

        app.next_panel(); // DiffView
        app.scroll_diff_down();
        assert_eq!(app.diff_scroll, 10);
    }

    #[test]
    fn test_scroll_diff_to_end() {
        let commits = create_test_commits();
        let mut files_map = HashMap::new();
        // Create a file with a patch containing 25 lines
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
            commits,
            files_map,
        );
        app.focused_panel = Panel::DiffView;

        app.scroll_diff_to_end();
        // 25 lines - 10 (visible) = 15
        assert_eq!(app.diff_scroll, 15);
    }

    #[test]
    fn test_file_change_resets_scroll() {
        let commits = create_test_commits();
        let files_map = create_test_files_map(&commits);
        let mut app = App::new(
            1,
            "owner/repo".to_string(),
            "Test PR".to_string(),
            commits,
            files_map,
        );
        app.focused_panel = Panel::DiffView;
        app.diff_scroll = 50;

        // Change to FileTree and select next file
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.select_next();

        // Scroll should be reset
        assert_eq!(app.diff_scroll, 0);
    }
}
