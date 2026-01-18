use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    widgets::{Block, Borders, Paragraph},
};

pub struct App {
    should_quit: bool,
    counter: u32,
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            counter: 0,
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
        let block = Block::default()
            .title(" prism - PR Review TUI ")
            .borders(Borders::ALL);

        let text = format!(
            "Hello, prism! 🔷\n\nArguments: {:?}\n\nCounter: {}\n\nPress 'j' to increment, 'k' to decrement, 'q' to quit",
            std::env::args().collect::<Vec<_>>(),
            self.counter
        );
        let paragraph = Paragraph::new(text).block(block);
        frame.render_widget(paragraph, area);
    }

    fn handle_events(&mut self) -> Result<()> {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => self.should_quit = true,
                    KeyCode::Char('j') => self.counter += 1,
                    KeyCode::Char('k') => self.counter = self.counter.saturating_sub(1),
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
