//! Cortex TUI — a thin terminal client over the Cortex HTTP API.
//!
//! Detects the current project from the working directory, scopes chat to it,
//! and talks to the existing `/api/*` endpoints. Auth uses a Clerk JWT bearer
//! token (env `CORTEX_TOKEN` or `~/.config/cortex/config.json`); with no token
//! the server's local/dev fallback applies.
//!
//! Usage:
//!   cortex                 launch the TUI in the current project
//!   cortex login <token>   persist an auth token
//!   cortex logout          clear the stored token
//!   cortex --help          show help

mod api;
mod app;
mod config;
mod files;
mod ui;

use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::api::ApiClient;
use crate::app::{App, AppEvent, Focus};
use crate::config::{Config, StoredConfig};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help") | Some("-h") => {
            print_help();
            return Ok(());
        }
        Some("login") => return cmd_login(args.get(1)),
        Some("logout") => return cmd_logout(),
        _ => {}
    }

    let config = Config::resolve().context("resolving configuration")?;
    let client = Arc::new(ApiClient::new(config.clone()).context("building API client")?);

    // Probe connectivity up front so failures are reported on a normal terminal.
    if let Err(e) = client.health().await {
        eprintln!("cortex: cannot reach API at {} — {e}", config.api_base);
        eprintln!("set CORTEX_API or run `cortex login <token>` to configure access.");
        std::process::exit(1);
    }

    run_tui(client).await
}

fn print_help() {
    println!(
        "cortex — terminal client for the Cortex API\n\n\
         USAGE:\n  \
         cortex                 launch the TUI scoped to the current directory\n  \
         cortex login <token>   store a Clerk JWT for authenticated access\n  \
         cortex logout          remove the stored token\n  \
         cortex --help          show this help\n\n\
         ENVIRONMENT:\n  \
         CORTEX_API     override the API base URL (default https://api.heyvera.org)\n  \
         CORTEX_TOKEN   bearer token, overrides the stored config"
    );
}

fn cmd_login(token: Option<&String>) -> Result<()> {
    let token = token.context("usage: cortex login <token>")?;
    let mut stored = StoredConfig::load();
    stored.token = Some(token.trim().to_string());
    stored.save()?;
    println!("cortex: token saved.");
    Ok(())
}

fn cmd_logout() -> Result<()> {
    let mut stored = StoredConfig::load();
    stored.token = None;
    stored.save()?;
    println!("cortex: token cleared.");
    Ok(())
}

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("creating terminal")
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

async fn run_tui(client: Arc<ApiClient>) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, client).await;
    restore_terminal(&mut terminal)?;
    result
}

async fn event_loop(terminal: &mut Tui, client: Arc<ApiClient>) -> Result<()> {
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut app = App::new(client, events_tx);
    app.refresh_runs();

    let mut input = EventStream::new();
    let mut redraw = true;
    // Periodic refresh of the task list.
    let mut ticker = tokio::time::interval(Duration::from_secs(10));

    loop {
        if redraw {
            terminal.draw(|f| ui::draw(f, &app))?;
            redraw = false;
        }

        tokio::select! {
            // Background events (chat stream, run refresh, status updates).
            Some(event) = events_rx.recv() => {
                app.handle_event(event);
                redraw = true;
            }
            // Terminal input.
            maybe_event = input.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                        handle_key(&mut app, key.code, key.modifiers);
                        redraw = true;
                    }
                    Some(Ok(Event::Resize(_, _))) => redraw = true,
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                    _ => {}
                }
            }
            _ = ticker.tick() => {
                app.refresh_runs();
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    // Global: Ctrl-C / Ctrl-Q quits from anywhere.
    if mods.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('c') | KeyCode::Char('q'))
    {
        app.should_quit = true;
        return;
    }

    if code == KeyCode::Tab {
        app.focus = app.focus.next();
        return;
    }

    match app.focus {
        Focus::Chat => match code {
            KeyCode::Enter => app.submit_message(),
            KeyCode::Char(c) => app.input.push(c),
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Esc => app.input.clear(),
            _ => {}
        },
        Focus::Files => match code {
            KeyCode::Up | KeyCode::Char('k') => app.files.move_up(),
            KeyCode::Down | KeyCode::Char('j') => app.files.move_down(),
            KeyCode::Enter => app.files.enter(),
            KeyCode::Char(' ') => app.toggle_attach_selected(),
            _ => {}
        },
        Focus::Tasks => match code {
            KeyCode::Up | KeyCode::Char('k') => {
                app.runs_selected = app.runs_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if app.runs_selected + 1 < app.runs.len() {
                    app.runs_selected += 1;
                }
            }
            KeyCode::Char('r') => app.refresh_runs(),
            _ => {}
        },
    }
}
