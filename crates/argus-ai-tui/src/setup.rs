//! Interactive `argus init` setup flow.

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};

const OUTPUT_PATH: &str = "argus.toml";

/// Configurable bootstrap settings edited by `argus init`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupConfig {
    pub socket_path: String,
    pub state_path: String,
    pub environment_name: String,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            socket_path: "/run/argus/argusd.sock".into(),
            state_path: "/var/lib/argus/argus.db".into(),
            environment_name: "default".into(),
        }
    }
}

const FIELDS: [&str; 3] = ["Socket path", "State path", "Environment name"];

struct SetupState {
    config: SetupConfig,
    focused: usize,
    message: Option<String>,
}

impl SetupState {
    fn field(&self, i: usize) -> &str {
        match i {
            0 => &self.config.socket_path,
            1 => &self.config.state_path,
            _ => &self.config.environment_name,
        }
    }

    fn field_mut(&mut self, i: usize) -> &mut String {
        match i {
            0 => &mut self.config.socket_path,
            1 => &mut self.config.state_path,
            _ => &mut self.config.environment_name,
        }
    }
}

/// Runs the interactive setup form and writes the result to `argus.toml`.
pub fn run(defaults: SetupConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = SetupState {
        config: defaults,
        focused: 0,
        message: None,
    };

    let result = loop {
        terminal.draw(|f| draw(f, &state))?;

        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && c == 's' => {
                    if let Err(e) = save(&state.config) {
                        state.message = Some(format!("Save failed: {e}"));
                    } else {
                        break Ok(());
                    }
                }
                KeyCode::Enter => {
                    if let Err(e) = save(&state.config) {
                        state.message = Some(format!("Save failed: {e}"));
                    } else {
                        break Ok(());
                    }
                }
                KeyCode::Char(c) => state.field_mut(state.focused).push(c),
                KeyCode::Backspace => {
                    state.field_mut(state.focused).pop();
                }
                KeyCode::Tab | KeyCode::Down => state.focused = (state.focused + 1) % FIELDS.len(),
                KeyCode::Up => state.focused = state.focused.saturating_sub(1),
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

fn save(config: &SetupConfig) -> Result<()> {
    let content = toml::to_string(config).context("serialize config")?;
    std::fs::write(OUTPUT_PATH, content).with_context(|| format!("write {OUTPUT_PATH}"))?;
    Ok(())
}

fn draw(f: &mut Frame, state: &SetupState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(f.area());

    f.render_widget(
        Paragraph::new("ARGUS Setup — edit bootstrap configuration (saved to argus.toml)")
            .style(Style::default().fg(Color::Cyan)),
        chunks[0],
    );

    for (i, label) in FIELDS.iter().enumerate() {
        let style = if i == state.focused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let line = Line::from(vec![
            Span::styled(format!("{label}: "), Style::default().fg(Color::Cyan)),
            Span::styled(state.field(i).to_string(), style),
        ]);
        f.render_widget(
            Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
            chunks[i + 1],
        );
    }

    let hint = "Tab/↑↓ navigate · type to edit · Enter or Ctrl-S to save · q/Esc to cancel";
    let message = state.message.clone().unwrap_or_default();
    f.render_widget(Paragraph::new(format!("{hint}\n{message}")), chunks[4]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_config_toml_round_trip() {
        let config = SetupConfig {
            socket_path: "/tmp/argus.sock".into(),
            state_path: "/tmp/argus.db".into(),
            environment_name: "prod".into(),
        };
        let serialized = toml::to_string(&config).unwrap();
        let back: SetupConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, back);
    }
}
