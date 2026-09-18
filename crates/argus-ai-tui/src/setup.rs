//! First-run setup wizard for ARGUS.

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};

use argus_ai_core::model::{ModelProviderConfig, ProviderCatalog};

const OUTPUT_PATH: &str = "argus.toml";
const SECRETS_PATH: &str = "argus.secrets.toml";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SecretsFile {
    api_token: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    Environment,
    Provider,
    Model,
    Storage,
    Review,
}

const STEPS: [Step; 6] = [
    Step::Welcome,
    Step::Environment,
    Step::Provider,
    Step::Model,
    Step::Storage,
    Step::Review,
];

impl Step {
    fn title(self) -> &'static str {
        match self {
            Step::Welcome => "Welcome",
            Step::Environment => "Environment",
            Step::Provider => "AI provider",
            Step::Model => "Model",
            Step::Storage => "Storage",
            Step::Review => "Review",
        }
    }

    fn index(self) -> usize {
        STEPS.iter().position(|s| *s == self).unwrap_or(0)
    }
}

struct SetupState {
    config: SetupConfig,
    api_token: String,
    step: Step,
    field: usize,
    provider_index: usize,
    message: Option<String>,
}

impl SetupState {
    fn field_count(&self) -> usize {
        match self.step {
            Step::Environment | Step::Provider => 1,
            Step::Model => 3,
            Step::Storage => 2,
            Step::Welcome | Step::Review => 0,
        }
    }

    fn field_value(&self) -> String {
        match (self.step, self.field) {
            (Step::Environment, 0) => self.config.environment_name.clone(),
            (Step::Provider, 0) => self.config.model.provider.clone(),
            (Step::Model, 0) => self.config.model.model.clone(),
            (Step::Model, 1) => self.config.model.fallback_models.join(", "),
            (Step::Model, 2) => self.api_token.clone(),
            (Step::Storage, 0) => self.config.socket_path.clone(),
            (Step::Storage, 1) => self.config.state_path.clone(),
            _ => String::new(),
        }
    }

    fn set_field_value(&mut self, value: String) {
        match (self.step, self.field) {
            (Step::Environment, 0) => self.config.environment_name = value,
            (Step::Provider, 0) => self.config.model.provider = value,
            (Step::Model, 0) => self.config.model.model = value,
            (Step::Model, 1) => self.config.model.fallback_models = parse_list(&value),
            (Step::Model, 2) => self.api_token = value,
            (Step::Storage, 0) => self.config.socket_path = value,
            (Step::Storage, 1) => self.config.state_path = value,
            _ => {}
        }
    }
}

pub fn run(defaults: SetupConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let provider_index = ProviderCatalog::all()
        .iter()
        .position(|p| p.id == defaults.model.provider)
        .unwrap_or(0);
    let mut state = SetupState {
        config: defaults,
        api_token: String::new(),
        step: Step::Welcome,
        field: 0,
        provider_index,
        message: None,
    };

    let result = loop {
        terminal.draw(|f| draw(f, &state))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                KeyCode::Left => previous_step(&mut state),
                KeyCode::Right | KeyCode::Tab | KeyCode::Enter => {
                    if state.step == Step::Review {
                        match save(&state) {
                            Ok(()) => break Ok(()),
                            Err(e) => state.message = Some(format!("{e:#}")),
                        }
                    } else {
                        next_step(&mut state);
                    }
                }
                KeyCode::Up => {
                    if state.step == Step::Provider {
                        select_provider(&mut state, -1);
                    } else {
                        move_field(&mut state, -1);
                    }
                }
                KeyCode::Down => {
                    if state.step == Step::Provider {
                        select_provider(&mut state, 1);
                    } else {
                        move_field(&mut state, 1);
                    }
                }
                KeyCode::Backspace => {
                    if state.field_count() > 0 {
                        let mut value = state.field_value();
                        value.pop();
                        state.set_field_value(value);
                    }
                }
                KeyCode::Char(c) if state.step != Step::Provider && state.field_count() > 0 => {
                    let mut value = state.field_value();
                    value.push(c);
                    state.set_field_value(value);
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

fn next_step(state: &mut SetupState) {
    let current = state.step.index();
    if current + 1 < STEPS.len() {
        state.step = STEPS[current + 1];
        state.field = 0;
        state.message = None;
    }
}

fn previous_step(state: &mut SetupState) {
    let current = state.step.index();
    if current > 0 {
        state.step = STEPS[current - 1];
        state.field = 0;
        state.message = None;
    }
}

fn move_field(state: &mut SetupState, direction: i32) {
    let count = state.field_count();
    if count == 0 {
        return;
    }
    let next = state.field as i32 + direction;
    state.field = next.clamp(0, (count - 1) as i32) as usize;
}

fn select_provider(state: &mut SetupState, direction: i32) {
    let providers = ProviderCatalog::all();
    if providers.is_empty() {
        return;
    }
    let next = state.provider_index as i32 + direction;
    state.provider_index = next.rem_euclid(providers.len() as i32) as usize;
    let provider = &providers[state.provider_index];
    state.config.model.provider = provider.id.to_owned();
    state.config.model.base_url = provider.default_base_url.map(str::to_owned);
    if state.config.model.model.is_empty() {
        state.config.model.model = provider.default_model.to_owned();
    }
}

fn parse_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
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
    let root = centered(f.area(), 88, 88);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(root);
    draw_header(f, chunks[0]);
    draw_stepper(f, chunks[1], state.step);
    draw_step(f, chunks[2], state);
    draw_footer(f, chunks[3], state);
}

fn draw_header(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(vec![
            Span::styled("◉ ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "ARGUS",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  First-time setup", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from("Configure the local runtime and AI provider."),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_stepper(f: &mut Frame, area: Rect, current: Step) {
    let spans = STEPS
        .iter()
        .enumerate()
        .flat_map(|(i, step)| {
            let style = if i < current.index() {
                Style::default().fg(Color::Green)
            } else if i == current.index() {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            [
                Span::styled(
                    format!(
                        " {} {} ",
                        if i < current.index() { "✓" } else { "•" },
                        step.title()
                    ),
                    style,
                ),
                Span::styled(
                    if i + 1 < STEPS.len() { "─" } else { "" },
                    Style::default().fg(Color::DarkGray),
                ),
            ]
        })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_step(f: &mut Frame, area: Rect, state: &SetupState) {
    match state.step {
        Step::Welcome => draw_welcome(f, area),
        Step::Environment => draw_form(
            f,
            area,
            "Environment",
            "Give this ARGUS installation a name.",
            vec![("Name", state.config.environment_name.clone(), false)],
            state.field,
        ),
        Step::Provider => draw_provider(f, area, state),
        Step::Model => draw_form(
            f,
            area,
            "Model",
            "Choose a primary model, optional fallbacks, and the provider credential.",
            vec![
                ("Model", state.config.model.model.clone(), false),
                (
                    "Fallbacks",
                    state.config.model.fallback_models.join(", "),
                    false,
                ),
                ("API token", state.api_token.clone(), true),
            ],
            state.field,
        ),
        Step::Storage => draw_form(
            f,
            area,
            "Storage",
            "Review the local runtime paths. Defaults are suitable for a standard install.",
            vec![
                ("Socket", state.config.socket_path.clone(), false),
                ("State", state.config.state_path.clone(), false),
            ],
            state.field,
        ),
        Step::Review => draw_review(f, area, state),
    }
}

fn draw_welcome(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Get started ")
        .border_style(Style::default().fg(Color::DarkGray));
    let lines = vec![
        Line::from("ARGUS is ready to be configured."),
        Line::from(""),
        Line::from("You will configure the environment, AI provider, model, and local storage."),
        Line::from(""),
        Line::from(Span::styled(
            "Secrets are stored separately from the main configuration.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter to continue.",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_provider(f: &mut Frame, area: Rect, state: &SetupState) {
    let providers = ProviderCatalog::all();
    let selected = providers.get(state.provider_index);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" AI provider ")
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines = vec![
        Line::from(Span::styled(
            "Select the AI provider ARGUS will use.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
    ];
    for (i, provider) in providers.iter().enumerate() {
        let style = if i == state.provider_index {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(vec![
            Span::styled(
                if i == state.provider_index {
                    "› "
                } else {
                    "  "
                },
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(provider.name, style),
            Span::styled(
                format!("  {}", provider.description),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    if let Some(provider) = selected {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "Base URL: {}",
                provider.default_base_url.unwrap_or("provider default")
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default())
            .wrap(Wrap { trim: true }),
        inner,
    );
}

fn draw_form(
    f: &mut Frame,
    area: Rect,
    title: &str,
    description: &str,
    fields: Vec<(&str, String, bool)>,
    focused: usize,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut constraints = vec![Constraint::Length(2)];
    constraints.extend(fields.iter().map(|_| Constraint::Length(4)));
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    f.render_widget(
        Paragraph::new(description)
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true }),
        body[0],
    );
    for (i, (label, value, secret)) in fields.iter().enumerate() {
        let style = if i == focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let displayed = if *secret && !value.is_empty() {
            "********".to_string()
        } else {
            value.clone()
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(*label, Style::default().fg(Color::DarkGray))),
                Line::from(Span::styled(displayed, style)),
            ])
            .block(Block::default().borders(Borders::BOTTOM)),
            body[i + 1],
        );
    }
}

fn draw_review(f: &mut Frame, area: Rect, state: &SetupState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Review & save ")
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let provider = if state.config.model.provider.is_empty() {
        "not configured"
    } else {
        &state.config.model.provider
    };
    let model = if state.config.model.model.is_empty() {
        "not configured"
    } else {
        &state.config.model.model
    };
    let lines = vec![
        labeled("Environment", &state.config.environment_name),
        labeled("Provider", provider),
        labeled("Model", model),
        labeled(
            "API token",
            if state.api_token.is_empty() {
                "not set"
            } else {
                "stored separately"
            },
        ),
        labeled("Socket", &state.config.socket_path),
        labeled("State", &state.config.state_path),
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter to save.",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "← goes back. q/Esc cancels without writing.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn labeled(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<12}"), Style::default().fg(Color::DarkGray)),
        Span::raw(value.to_owned()),
    ])
}

fn draw_footer(f: &mut Frame, area: Rect, state: &SetupState) {
    let hint = match state.step {
        Step::Welcome => "Enter continue   q/Esc cancel",
        Step::Review => "Enter save   ← back   q/Esc cancel",
        _ => "↑↓ select   type edit   Tab/→ next   ← back   q/Esc cancel",
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(hint, Style::default().fg(Color::DarkGray)),
            Span::styled(
                state.message.as_deref().unwrap_or(""),
                Style::default().fg(Color::Red),
            ),
        ])),
        area,
    );
}

fn centered(area: Rect, width: u16, height_percent: u16) -> Rect {
    let width = width.min(area.width);
    let height = (area.height * height_percent / 100)
        .max(16)
        .min(area.height);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(horizontal[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_list_parsing_trims_and_drops_empty() {
        assert_eq!(
            parse_list("gpt-4.1-mini, llama3.1 , "),
            vec!["gpt-4.1-mini", "llama3.1"]
        );
    }
}
