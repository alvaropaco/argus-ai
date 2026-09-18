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

use argus_ai_core::model::ModelProviderConfig;

const OUTPUT_PATH: &str = "argus.toml";
const SECRETS_PATH: &str = "argus.secrets.toml";

/// Configurable bootstrap settings edited by `argus init`.
///
/// Non-secret values only. The provider API token is written to a separate
/// mode-`0600` file (see [`write_secrets`]) so it never lands in plaintext
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupConfig {
    pub socket_path: String,
    pub state_path: String,
    pub environment_name: String,
    #[serde(default)]
    pub model: ModelProviderConfig,
}

impl Default for SetupConfig {
    fn default() -> Self {
        Self {
            socket_path: "/run/argus/argusd.sock".into(),
            state_path: "/var/lib/argus/argus.db".into(),
            environment_name: "default".into(),
            model: ModelProviderConfig::default(),
        }
    }
}

/// Secret material collected by `argus init`, persisted separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SecretsFile {
    api_token: String,
}

struct FieldDef {
    label: &'static str,
    secret: bool,
}

const FIELDS: [FieldDef; 8] = [
    FieldDef {
        label: "Socket path",
        secret: false,
    },
    FieldDef {
        label: "State path",
        secret: false,
    },
    FieldDef {
        label: "Environment name",
        secret: false,
    },
    FieldDef {
        label: "AI provider (e.g. openai, anthropic, ollama)",
        secret: false,
    },
    FieldDef {
        label: "Model",
        secret: false,
    },
    FieldDef {
        label: "Fallback models (comma-separated)",
        secret: false,
    },
    FieldDef {
        label: "API base URL (optional)",
        secret: false,
    },
    FieldDef {
        label: "API token",
        secret: true,
    },
];

struct SetupState {
    config: SetupConfig,
    api_token: String,
    focused: usize,
    message: Option<String>,
}

impl SetupState {
    fn field(&self, i: usize) -> String {
        match i {
            0 => self.config.socket_path.clone(),
            1 => self.config.state_path.clone(),
            2 => self.config.environment_name.clone(),
            3 => self.config.model.provider.clone(),
            4 => self.config.model.model.clone(),
            5 => self.config.model.fallback_models.join(", "),
            6 => self.config.model.base_url.clone().unwrap_or_default(),
            _ => self.api_token.clone(),
        }
    }

    fn set_field(&mut self, i: usize, value: String) {
        match i {
            0 => self.config.socket_path = value,
            1 => self.config.state_path = value,
            2 => self.config.environment_name = value,
            3 => self.config.model.provider = value,
            4 => self.config.model.model = value,
            5 => self.config.model.fallback_models = parse_list(&value),
            6 => self.config.model.base_url = if value.is_empty() { None } else { Some(value) },
            _ => self.api_token = value,
        }
    }
}

fn parse_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
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
        api_token: String::new(),
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
                    if let Err(e) = save(&state) {
                        state.message = Some(format!("Save failed: {e}"));
                    } else {
                        break Ok(());
                    }
                }
                KeyCode::Enter => {
                    if let Err(e) = save(&state) {
                        state.message = Some(format!("Save failed: {e}"));
                    } else {
                        break Ok(());
                    }
                }
                KeyCode::Char(c) => {
                    let mut value = state.field(state.focused);
                    value.push(c);
                    state.set_field(state.focused, value);
                }
                KeyCode::Backspace => {
                    let mut value = state.field(state.focused);
                    value.pop();
                    state.set_field(state.focused, value);
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

fn save(state: &SetupState) -> Result<()> {
    let content = toml::to_string(&state.config).context("serialize config")?;
    std::fs::write(OUTPUT_PATH, content).with_context(|| format!("write {OUTPUT_PATH}"))?;

    if !state.api_token.is_empty() {
        write_secrets(&state.api_token)?;
    }
    Ok(())
}

fn write_secrets(api_token: &str) -> Result<()> {
    let content = toml::to_string(&SecretsFile {
        api_token: api_token.to_owned(),
    })
    .context("serialize secrets")?;
    std::fs::write(SECRETS_PATH, content).with_context(|| format!("write {SECRETS_PATH}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(SECRETS_PATH, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {SECRETS_PATH}"))?;
    }
    Ok(())
}

fn draw(f: &mut Frame, state: &SetupState) {
    let mut constraints = vec![Constraint::Length(2)];
    for _ in 0..FIELDS.len() {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Min(0));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.area());

    f.render_widget(
        Paragraph::new(
            "ARGUS Setup — bootstrap configuration + AI provider (saved to argus.toml, token to argus.secrets.toml)",
        )
        .style(Style::default().fg(Color::Cyan)),
        chunks[0],
    );

    for (i, field) in FIELDS.iter().enumerate() {
        let style = if i == state.focused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let raw = state.field(i);
        let value = if field.secret {
            "*".repeat(raw.chars().count())
        } else {
            raw
        };
        let line = Line::from(vec![
            Span::styled(
                format!("{}: ", field.label),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(value, style),
        ]);
        f.render_widget(
            Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
            chunks[i + 1],
        );
    }

    let hint = "Tab/↑↓ navigate · type to edit · Enter or Ctrl-S to save · q/Esc to cancel";
    let message = state.message.clone().unwrap_or_default();
    f.render_widget(
        Paragraph::new(format!("{hint}\n{message}")),
        chunks[FIELDS.len() + 1],
    );
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
            model: ModelProviderConfig {
                provider: "openai".into(),
                model: "gpt-4.1".into(),
                fallback_models: vec!["gpt-4.1-mini".into()],
                base_url: None,
            },
        };
        let serialized = toml::to_string(&config).unwrap();
        let back: SetupConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn fallback_list_parsing_trims_and_drops_empty() {
        assert_eq!(
            parse_list("gpt-4.1-mini, llama3.1 , "),
            vec!["gpt-4.1-mini", "llama3.1"]
        );
        assert_eq!(parse_list(""), Vec::<String>::new());
    }

    #[test]
    fn secrets_serialize_without_config() {
        let secrets = SecretsFile {
            api_token: "sk-test".into(),
        };
        let toml = toml::to_string(&secrets).unwrap();
        let back: SecretsFile = toml::from_str(&toml).unwrap();
        assert_eq!(secrets, back);
        assert!(!toml.contains("socket_path"));
    }

    #[test]
    fn set_field_round_trips_through_state() {
        let mut state = SetupState {
            config: SetupConfig::default(),
            api_token: String::new(),
            focused: 0,
            message: None,
        };
        state.set_field(3, "ollama".into());
        state.set_field(4, "llama3.1".into());
        state.set_field(5, "llama3.1, mistral".into());
        state.set_field(6, "http://localhost:11434".into());
        state.set_field(7, "secret-token".into());

        assert_eq!(state.field(3), "ollama");
        assert_eq!(state.field(4), "llama3.1");
        assert_eq!(state.field(5), "llama3.1, mistral");
        assert_eq!(state.field(6), "http://localhost:11434");
        assert_eq!(state.field(7), "secret-token");
    }
}
