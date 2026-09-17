//! Ratatui application shell: tabs for each bootstrap view.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use ratatui::{Frame, Terminal};

use crate::data::Data;

const TABS: [&str; 5] = ["Health", "Status", "Capabilities", "Plugins", "Config"];

pub fn run(data: Data) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut selected = 0usize;
    let result = loop {
        terminal.draw(|f| draw(f, &data, selected))?;

        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                KeyCode::Tab | KeyCode::Right => selected = (selected + 1) % TABS.len(),
                KeyCode::Left => selected = selected.saturating_sub(1),
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

fn draw(f: &mut Frame, data: &Data, selected: usize) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(f.area());

    let titles = TABS.iter().map(|t| Line::from(*t)).collect::<Vec<_>>();
    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::default().fg(Color::Cyan))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, chunks[0]);

    let body = match selected {
        0 => pretty(&data.health),
        1 => pretty(&data.status),
        2 => pretty(&data.capabilities),
        3 => pretty(&data.plugins),
        _ => pretty(&data.config),
    };
    let paragraph =
        Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(TABS[selected]));
    f.render_widget(paragraph, chunks[1]);
}

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
}
