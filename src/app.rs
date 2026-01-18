use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
};

pub struct App {
    should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self { should_quit: false }
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
        let block = Block::default().title(" Commits ").borders(Borders::ALL);
        let paragraph = Paragraph::new("commit list").block(block);
        frame.render_widget(paragraph, area);
    }

    fn draw_file_tree(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().title(" Files ").borders(Borders::ALL);
        let paragraph = Paragraph::new("file tree").block(block);
        frame.render_widget(paragraph, area);
    }

    fn draw_diff_view(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().title(" Diff ").borders(Borders::ALL);
        let paragraph = Paragraph::new("diff view").block(block);
        frame.render_widget(paragraph, area);
    }

    fn handle_events(&mut self) -> Result<()> {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => self.should_quit = true,
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
