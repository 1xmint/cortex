//! Rendering for the Cortex TUI. Three-panel layout:
//!   left: chat transcript + input box
//!   right top: project file browser
//!   right bottom: task/run status

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus, Role};

pub fn draw(frame: &mut Frame, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // main body
            Constraint::Length(1), // status bar
        ])
        .split(frame.area());

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(root[0]);

    draw_chat(frame, app, body[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(body[1]);

    draw_files(frame, app, right[0]);
    draw_tasks(frame, app, right[1]);
    draw_status(frame, app, root[1]);
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn draw_chat(frame: &mut Frame, app: &App, area: Rect) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let mut lines: Vec<Line> = Vec::new();
    for msg in &app.messages {
        let (label, color) = match msg.role {
            Role::User => ("you", Color::Green),
            Role::Assistant => ("cortex", Color::Cyan),
            Role::System => ("system", Color::Yellow),
        };
        lines.push(Line::from(Span::styled(
            format!("{label}:"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
        for text_line in msg.text.split('\n') {
            lines.push(Line::from(text_line.to_string()));
        }
        lines.push(Line::from(""));
    }

    // Keep the latest content visible by scrolling to the bottom.
    let inner_height = layout[0].height.saturating_sub(2) as usize;
    let scroll = lines.len().saturating_sub(inner_height) as u16;

    let transcript = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style(app.focus == Focus::Chat))
                .title(" chat "),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(transcript, layout[0]);

    let prompt = if app.streaming { "…" } else { ">" };
    let input = Paragraph::new(format!("{prompt} {}", app.input)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style(app.focus == Focus::Chat))
            .title(" message "),
    );
    frame.render_widget(input, layout[1]);
}

fn draw_files(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .files
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let attached = app
                .files
                .selected_relative_file_for(entry)
                .map(|rel| app.attached.iter().any(|a| a == &rel))
                .unwrap_or(false);

            let marker = if entry.is_dir {
                "/"
            } else if attached {
                "*"
            } else {
                " "
            };
            let label = format!("{marker} {}", entry.name);
            let style = if i == app.files.selected && app.focus == Focus::Files {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else if entry.is_dir {
                Style::default().fg(Color::Blue)
            } else if attached {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        })
        .collect();

    let title = format!(" files: {} ", app.files.cwd_display());
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style(app.focus == Focus::Files))
            .title(title),
    );
    frame.render_widget(list, area);
}

fn draw_tasks(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = if app.runs.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "no recent tasks (press r to refresh)",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        app.runs
            .iter()
            .enumerate()
            .map(|(i, run)| {
                let color = match run.status.as_str() {
                    "completed" | "succeeded" | "success" => Color::Green,
                    "failed" | "error" => Color::Red,
                    "running" | "in_progress" => Color::Yellow,
                    _ => Color::Gray,
                };
                let goal = if run.goal.is_empty() {
                    run.id.clone()
                } else {
                    run.goal.clone()
                };
                let label = format!("[{}] {}", run.status, truncate(&goal, 40));
                let style = if i == app.runs_selected && app.focus == Focus::Tasks {
                    Style::default().fg(Color::Black).bg(color)
                } else {
                    Style::default().fg(color)
                };
                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect()
    };

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style(app.focus == Focus::Tasks))
            .title(" tasks "),
    );
    frame.render_widget(list, area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let attached = if app.attached.is_empty() {
        String::new()
    } else {
        format!("  ctx:{}", app.attached.len())
    };
    let help = match app.focus {
        Focus::Chat => "Enter send  Tab panel  Ctrl-C quit",
        Focus::Files => "↑↓ move  Enter open  Space attach  Tab panel",
        Focus::Tasks => "↑↓ move  r refresh  Tab panel",
    };
    let text = format!(" {} | {}{}", app.status, help, attached);
    let bar = Paragraph::new(text).style(Style::default().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(bar, area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
