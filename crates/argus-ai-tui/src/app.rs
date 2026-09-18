//! Product-oriented ARGUS dashboard TUI.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::data::Data;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Health,
    Capabilities,
    Plugins,
    Config,
}

const VIEWS: [(&str, &str); 5] = [
    ("1", "Overview"),
    ("2", "Health"),
    ("3", "Capabilities"),
    ("4", "Plugins"),
    ("5", "Config"),
];

pub fn run(data: Data) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut view = View::Overview;

    let result = loop {
        terminal.draw(|f| draw(f, &data, view))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                KeyCode::Char('1') => view = View::Overview,
                KeyCode::Char('2') => view = View::Health,
                KeyCode::Char('3') => view = View::Capabilities,
                KeyCode::Char('4') => view = View::Plugins,
                KeyCode::Char('5') => view = View::Config,
                KeyCode::Tab | KeyCode::Right => view = next_view(view),
                KeyCode::Left => view = previous_view(view),
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}

fn next_view(view: View) -> View {
    match view {
        View::Overview => View::Health,
        View::Health => View::Capabilities,
        View::Capabilities => View::Plugins,
        View::Plugins => View::Config,
        View::Config => View::Overview,
    }
}

fn previous_view(view: View) -> View {
    match view {
        View::Overview => View::Config,
        View::Health => View::Overview,
        View::Capabilities => View::Health,
        View::Plugins => View::Capabilities,
        View::Config => View::Plugins,
    }
}

fn draw(f: &mut Frame, data: &Data, view: View) {
    let root = centered(f.area(), 96, 92);
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
    draw_navigation(f, chunks[1], view);
    draw_content(f, chunks[2], data, view);
    draw_footer(f, chunks[3]);
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
            Span::styled(
                "  AI Infrastructure Operations",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from("Observe • Understand • Plan • Authorize • Execute • Validate"),
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

fn draw_navigation(f: &mut Frame, area: Rect, selected: View) {
    let active = label_for(selected);
    let line = VIEWS
        .iter()
        .flat_map(|(key, label)| {
            let style = if *label == active {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            [
                Span::styled(format!(" {key} "), Style::default().fg(Color::Yellow)),
                Span::styled(*label, style),
                Span::raw("   "),
            ]
        })
        .collect::<Vec<_>>();

    f.render_widget(Paragraph::new(Line::from(line)), area);
}

fn label_for(view: View) -> &'static str {
    match view {
        View::Overview => "Overview",
        View::Health => "Health",
        View::Capabilities => "Capabilities",
        View::Plugins => "Plugins",
        View::Config => "Config",
    }
}

fn draw_content(f: &mut Frame, area: Rect, data: &Data, view: View) {
    match view {
        View::Overview => draw_overview(f, area, data),
        View::Health => draw_json_card(f, area, "Health", &data.health),
        View::Capabilities => draw_json_card(f, area, "Capabilities", &data.capabilities),
        View::Plugins => draw_json_card(f, area, "Plugins", &data.plugins),
        View::Config => draw_json_card(f, area, "Configuration", &data.config),
    }
}

fn draw_overview(f: &mut Frame, area: Rect, data: &Data) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(area);
    let healthy = data.health.get("error").is_none();
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(rows[0]);

    card(
        f,
        cards[0],
        "RUNTIME",
        if healthy {
            "Operational"
        } else {
            "Unavailable"
        },
        if healthy {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        },
    );
    card(
        f,
        cards[1],
        "DAEMON",
        &value_or(&data.status, "version", "Connected"),
        Style::default().fg(Color::Cyan),
    );
    card(
        f,
        cards[2],
        "ENVIRONMENT",
        &value_or(&data.config, "environment_name", "Default"),
        Style::default().fg(Color::White),
    );

    let lower = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    draw_summary(f, lower[0], data);
    draw_help(f, lower[1]);
}

fn card(f: &mut Frame, area: Rect, title: &str, value: &str, style: Style) {
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                title,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(value, style.add_modifier(Modifier::BOLD))),
        ])
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_summary(f: &mut Frame, area: Rect, data: &Data) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" System snapshot ")
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let degraded = data.health.get("error").is_some();
    let items = vec![
        ListItem::new(Line::from(vec![
            Span::styled("Health       ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if degraded { "degraded" } else { "ready" },
                if degraded {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
        ])),
        ListItem::new(format!("Capabilities  {}", json_len(&data.capabilities))),
        ListItem::new(format!("Plugins       {}", json_len(&data.plugins))),
        ListItem::new(format!("Config keys   {}", json_len(&data.config))),
        ListItem::new(""),
        ListItem::new(Line::from(Span::styled(
            "Execution remains behind policy + executor boundaries.",
            Style::default().fg(Color::DarkGray),
        ))),
    ];
    f.render_widget(List::new(items), inner);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Navigation ")
        .border_style(Style::default().fg(Color::DarkGray));
    let help = vec![
        Line::from(vec![
            Span::styled("1–5", Style::default().fg(Color::Yellow)),
            Span::raw(" switch views"),
        ]),
        Line::from(vec![
            Span::styled("← → / Tab", Style::default().fg(Color::Yellow)),
            Span::raw(" navigate"),
        ]),
        Line::from(vec![
            Span::styled("q / Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" exit"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Use argus init for first-time setup.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(help).block(block), area);
}

fn draw_json_card(f: &mut Frame, area: Rect, title: &str, value: &serde_json::Value) {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        Paragraph::new(pretty)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_footer(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("ARGUS", Style::default().fg(Color::Cyan)),
            Span::styled(
                "  •  AI Infrastructure Operations",
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::TOP)),
        area,
    );
}

fn value_or(value: &serde_json::Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(fallback)
        .to_string()
}

fn json_len(value: &serde_json::Value) -> String {
    value
        .as_array()
        .map(|v| v.len().to_string())
        .or_else(|| value.as_object().map(|v| v.len().to_string()))
        .unwrap_or_else(|| "—".into())
}

fn centered(area: Rect, width: u16, height_percent: u16) -> Rect {
    let width = width.min(area.width);
    let height = (area.height * height_percent / 100)
        .max(12)
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
