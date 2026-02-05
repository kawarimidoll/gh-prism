use crate::github::commits::CommitInfo;
use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

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
}

impl App {
    pub fn new(pr_number: u64, repo: String, pr_title: String, commits: Vec<CommitInfo>) -> Self {
        let mut commit_list_state = ListState::default();
        if !commits.is_empty() {
            commit_list_state.select(Some(0));
        }
        Self {
            should_quit: false,
            focused_panel: Panel::CommitList,
            pr_number,
            repo,
            pr_title,
            commits,
            commit_list_state,
        }
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
            " prism - {} PR #{}: {} | Tab: switch | j/k: select | q: quit",
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
        self.render_file_tree_widget(frame, sidebar_layout[1]);
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

    fn render_file_tree_widget(&self, frame: &mut Frame, area: Rect) {
        let style = if self.focused_panel == Panel::FileTree {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        let block = Block::default()
            .title(" Files ")
            .borders(Borders::ALL)
            .border_style(style);
        let paragraph = Paragraph::new("file tree").block(block);
        frame.render_widget(paragraph, area);
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
        let paragraph = Paragraph::new("diff view").block(block);
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
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn select_next(&mut self) {
        if self.focused_panel == Panel::CommitList && !self.commits.is_empty() {
            let current = self.commit_list_state.selected().unwrap_or(0);
            let next = (current + 1) % self.commits.len();
            self.commit_list_state.select(Some(next));
        }
    }

    fn select_prev(&mut self) {
        if self.focused_panel == Panel::CommitList && !self.commits.is_empty() {
            let current = self.commit_list_state.selected().unwrap_or(0);
            let prev = if current == 0 {
                self.commits.len() - 1
            } else {
                current - 1
            };
            self.commit_list_state.select(Some(prev));
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

    #[test]
    fn test_new_with_empty_commits() {
        let app = App::new(1, "owner/repo".to_string(), "Test PR".to_string(), vec![]);
        assert!(!app.should_quit);
        assert_eq!(app.focused_panel, Panel::CommitList);
        assert_eq!(app.pr_number, 1);
        assert_eq!(app.repo, "owner/repo");
        assert_eq!(app.pr_title, "Test PR");
        assert!(app.commits.is_empty());
        assert_eq!(app.commit_list_state.selected(), None);
    }

    #[test]
    fn test_new_with_commits() {
        let commits = create_test_commits();
        let app = App::new(1, "owner/repo".to_string(), "Test PR".to_string(), commits);
        assert_eq!(app.commits.len(), 2);
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    #[test]
    fn test_next_panel() {
        let mut app = App::new(1, "owner/repo".to_string(), "Test PR".to_string(), vec![]);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::DiffView);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_prev_panel() {
        let mut app = App::new(1, "owner/repo".to_string(), "Test PR".to_string(), vec![]);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::DiffView);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_select_next() {
        let commits = create_test_commits();
        let mut app = App::new(1, "owner/repo".to_string(), "Test PR".to_string(), commits);
        assert_eq!(app.commit_list_state.selected(), Some(0));
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(1));
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(0)); // wrap around
    }

    #[test]
    fn test_select_prev() {
        let commits = create_test_commits();
        let mut app = App::new(1, "owner/repo".to_string(), "Test PR".to_string(), commits);
        assert_eq!(app.commit_list_state.selected(), Some(0));
        app.select_prev();
        assert_eq!(app.commit_list_state.selected(), Some(1)); // wrap around
        app.select_prev();
        assert_eq!(app.commit_list_state.selected(), Some(0));
    }

    #[test]
    fn test_select_only_works_in_commit_list_panel() {
        let commits = create_test_commits();
        let mut app = App::new(1, "owner/repo".to_string(), "Test PR".to_string(), commits);
        app.next_panel(); // Move to FileTree
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.select_next();
        assert_eq!(app.commit_list_state.selected(), Some(0)); // Should not change
    }

    #[test]
    fn test_commit_list_state() {
        let commits = create_test_commits();
        let app = App::new(1, "owner/repo".to_string(), "Test PR".to_string(), commits);

        // Verify the commit list state is properly initialized
        assert_eq!(app.commit_list_state.selected(), Some(0));
        assert_eq!(app.commits.len(), 2);
        assert_eq!(app.commits[0].short_sha(), "abc1234");
        assert_eq!(app.commits[0].message_summary(), "First commit");
    }
}
