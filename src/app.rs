use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
};

#[derive(Clone, Copy, PartialEq, Eq)]
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
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let area = frame.area();

        let main_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let header = Paragraph::new(" prism - PR Review TUI ")
            .style(Style::default().bg(Color::Blue).fg(Color::White));
        frame.render_widget(header, main_layout[0]);

        let body_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(main_layout[1]);

        let sidebar_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(body_layout[0]);

        self.draw_commit_list(frame, sidebar_layout[0]);
        self.draw_file_tree(frame, sidebar_layout[1]);
        self.draw_diff_view(frame, body_layout[1]);
    }

    fn draw_commit_list(&self, frame: &mut Frame, area: Rect) {
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
        frame.render_widget(paragraph, area);
    }

    fn draw_file_tree(&self, frame: &mut Frame, area: Rect) {
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

    fn draw_diff_view(&self, frame: &mut Frame, area: Rect) {
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
