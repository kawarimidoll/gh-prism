use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Widget},
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
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            focused_panel: Panel::CommitList,
        }
    }

    pub fn run(&mut self, mut terminal: DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| frame.render_widget(&*self, frame.area()))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn render_commit_list(&self, area: Rect, buf: &mut Buffer) {
        let style = if self.focused_panel == Panel::CommitList {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        let block = Block::default()
            .title(" Commits ")
            .borders(Borders::ALL)
            .border_style(style);
        let paragraph = Paragraph::new("commit list").block(block);
        paragraph.render(area, buf);
    }

    fn render_file_tree(&self, area: Rect, buf: &mut Buffer) {
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
        paragraph.render(area, buf);
    }

    fn render_diff_view(&self, area: Rect, buf: &mut Buffer) {
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
        paragraph.render(area, buf);
    }

    fn handle_events(&mut self) -> Result<()> {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => self.should_quit = true,
                    KeyCode::Tab => self.next_panel(),
                    KeyCode::BackTab => self.prev_panel(),
                    _ => {}
                }
            }
        }
        Ok(())
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

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        Paragraph::new(" prism - PR Review TUI ")
            .style(Style::default().bg(Color::Blue).fg(Color::White))
            .render(main_layout[0], buf);

        let body_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(main_layout[1]);

        let sidebar_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(body_layout[0]);

        self.render_commit_list(sidebar_layout[0], buf);
        self.render_file_tree(sidebar_layout[1], buf);
        self.render_diff_view(body_layout[1], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let app = App::new();
        assert!(!app.should_quit);
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_next_panel() {
        let mut app = App::new();
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::DiffView);
        app.next_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_prev_panel() {
        let mut app = App::new();
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::DiffView);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::FileTree);
        app.prev_panel();
        assert_eq!(app.focused_panel, Panel::CommitList);
    }

    #[test]
    fn test_render_has_header() {
        let app = App::new();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        (&app).render(area, &mut buf);

        let header_text: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert!(header_text.contains("prism"));
    }
}
