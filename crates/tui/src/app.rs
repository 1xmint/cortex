//! Application state for the Cortex TUI.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::api::{ApiClient, ChatEvent, Run};
use crate::files::FileBrowser;

/// Which panel currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Chat,
    Files,
    Tasks,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Focus::Chat => Focus::Files,
            Focus::Files => Focus::Tasks,
            Focus::Tasks => Focus::Chat,
        }
    }
}

/// Role of a chat message for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

/// Messages emitted from background tasks back into the UI loop.
pub enum AppEvent {
    Chat(ChatEvent),
    ChatError(String),
    ChatDone,
    Runs(Vec<Run>),
    Status(String),
}

pub struct App {
    pub client: Arc<ApiClient>,
    pub focus: Focus,
    pub messages: Vec<Message>,
    pub input: String,
    pub files: FileBrowser,
    /// File paths (project-relative) attached as context for the next message.
    pub attached: Vec<String>,
    attached_set: HashSet<String>,
    pub runs: Vec<Run>,
    pub runs_selected: usize,
    pub streaming: bool,
    pub status: String,
    pub should_quit: bool,
    /// Index in `messages` of the in-progress assistant reply, if any.
    pending_assistant: Option<usize>,
    /// Sender the UI loop hands to background tasks.
    pub events_tx: mpsc::UnboundedSender<AppEvent>,
}

impl App {
    pub fn new(client: Arc<ApiClient>, events_tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        let project = &client.config().project;
        let files = FileBrowser::new(project.root.clone());

        let intro = if project.detected {
            format!(
                "Connected. Project scope: {} ({})",
                project.name,
                project.root.display()
            )
        } else {
            format!(
                "Connected. No project marker found; scoped to {}",
                project.root.display()
            )
        };

        let messages = vec![Message {
            role: Role::System,
            text: format!(
                "{intro}\nType a message and press Enter to chat. Tab switches panels, Ctrl-C quits."
            ),
        }];

        Self {
            client,
            focus: Focus::Chat,
            messages,
            input: String::new(),
            files,
            attached: Vec::new(),
            attached_set: HashSet::new(),
            runs: Vec::new(),
            runs_selected: 0,
            streaming: false,
            status: "ready".to_string(),
            should_quit: false,
            pending_assistant: None,
            events_tx,
        }
    }

    /// Toggle attachment of the currently-highlighted file as chat context.
    pub fn toggle_attach_selected(&mut self) {
        if let Some(rel) = self.files.selected_relative_file() {
            if self.attached_set.remove(&rel) {
                self.attached.retain(|p| p != &rel);
                self.status = format!("detached {rel}");
            } else {
                self.attached_set.insert(rel.clone());
                self.attached.push(rel.clone());
                self.status = format!("attached {rel}");
            }
        }
    }

    /// Submit the current input as a chat message, kicking off a streaming task.
    pub fn submit_message(&mut self) {
        let message = self.input.trim().to_string();
        if message.is_empty() || self.streaming {
            return;
        }
        self.input.clear();

        self.messages.push(Message {
            role: Role::User,
            text: message.clone(),
        });
        self.messages.push(Message {
            role: Role::Assistant,
            text: String::new(),
        });
        self.pending_assistant = Some(self.messages.len() - 1);
        self.streaming = true;
        self.status = "streaming…".to_string();

        let file_paths = self.attached.clone();
        let client = self.client.clone();
        let ui_tx = self.events_tx.clone();

        tokio::spawn(async move {
            let (chat_tx, mut chat_rx) = mpsc::unbounded_channel::<ChatEvent>();
            let forward = ui_tx.clone();
            let pump = tokio::spawn(async move {
                while let Some(event) = chat_rx.recv().await {
                    if forward.send(AppEvent::Chat(event)).is_err() {
                        break;
                    }
                }
            });

            let result = client.stream_chat(message, file_paths, None, chat_tx).await;
            let _ = pump.await;

            match result {
                Ok(()) => {
                    let _ = ui_tx.send(AppEvent::ChatDone);
                }
                Err(e) => {
                    let _ = ui_tx.send(AppEvent::ChatError(e.to_string()));
                }
            }
        });
    }

    /// Apply a background event to the UI state.
    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Chat(ChatEvent::Started { provider, model }) => {
                self.status = if model.is_empty() {
                    format!("streaming via {provider}")
                } else {
                    format!("streaming via {provider}/{model}")
                };
            }
            AppEvent::Chat(ChatEvent::Output { line }) => {
                if let Some(idx) = self.pending_assistant {
                    if let Some(msg) = self.messages.get_mut(idx) {
                        msg.text.push_str(&line);
                    }
                }
            }
            AppEvent::Chat(ChatEvent::Completed { .. }) => {
                self.finish_stream(None);
            }
            AppEvent::Chat(ChatEvent::Failed { error }) => {
                self.finish_stream(Some(error));
            }
            AppEvent::ChatDone => {
                self.finish_stream(None);
            }
            AppEvent::ChatError(error) => {
                self.finish_stream(Some(error));
            }
            AppEvent::Runs(runs) => {
                self.runs = runs;
                self.runs_selected = self.runs_selected.min(self.runs.len().saturating_sub(1));
            }
            AppEvent::Status(s) => {
                self.status = s;
            }
        }
    }

    fn finish_stream(&mut self, error: Option<String>) {
        if !self.streaming && error.is_none() {
            return;
        }
        self.streaming = false;
        if let Some(err) = error {
            // Replace an empty pending reply with the error; otherwise append.
            if let Some(idx) = self.pending_assistant {
                if let Some(msg) = self.messages.get_mut(idx) {
                    if msg.text.is_empty() {
                        msg.role = Role::System;
                        msg.text = format!("error: {err}");
                    } else {
                        self.messages.push(Message {
                            role: Role::System,
                            text: format!("error: {err}"),
                        });
                    }
                }
            }
            self.status = "error".to_string();
        } else {
            self.status = "ready".to_string();
        }
        self.pending_assistant = None;
    }

    /// Spawn a background refresh of the task/run list.
    pub fn refresh_runs(&self) {
        let client = self.client.clone();
        let tx = self.events_tx.clone();
        tokio::spawn(async move {
            match client.runs().await {
                Ok(runs) => {
                    let _ = tx.send(AppEvent::Runs(runs));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Status(format!("runs error: {e}")));
                }
            }
        });
    }
}
