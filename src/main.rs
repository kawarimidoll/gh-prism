use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    widgets::{Block, Borders, Paragraph},
};

fn main() -> Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = run(terminal);
    ratatui::restore();
    result
}

fn run(mut terminal: DefaultTerminal) -> Result<()> {
    loop {
        terminal.draw(render)?;
        if should_quit()? {
            break;
        }
    }
    Ok(())
}

fn render(frame: &mut Frame) {
    let area = frame.area();
    let block = Block::default()
        .title(" prism - PR Review TUI ")
        .borders(Borders::ALL);

    let text = format!(
        "Hello, prism! 🔷\n\nArguments: {:?}\n\nPress 'q' to quit",
        std::env::args().collect::<Vec<_>>()
    );
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

fn should_quit() -> Result<bool> {
    if let Event::Key(key) = event::read()? {
        if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
            return Ok(true);
        }
    }
    Ok(false)
}
