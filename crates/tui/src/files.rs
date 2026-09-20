//! Lightweight project file browser. Lists entries within the project root,
//! supporting navigation into subdirectories and selection of files to attach
//! as chat context (`file_paths`).

use std::path::{Path, PathBuf};

/// A single browsable entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Stateful file browser rooted at the project directory.
pub struct FileBrowser {
    root: PathBuf,
    cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
}

impl FileBrowser {
    pub fn new(root: PathBuf) -> Self {
        let mut browser = Self {
            cwd: root.clone(),
            root,
            entries: Vec::new(),
            selected: 0,
        };
        browser.refresh();
        browser
    }

    /// Re-read the current directory listing.
    pub fn refresh(&mut self) {
        let mut entries = Vec::new();

        // Allow stepping back up to (but not above) the project root.
        if self.cwd != self.root {
            if let Some(parent) = self.cwd.parent() {
                entries.push(Entry {
                    name: "..".to_string(),
                    path: parent.to_path_buf(),
                    is_dir: true,
                });
            }
        }

        if let Ok(read) = std::fs::read_dir(&self.cwd) {
            let mut listing: Vec<Entry> = read
                .filter_map(|e| e.ok())
                .filter(|e| {
                    // Skip hidden dotfiles and heavy build/dep dirs to keep it useful.
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    !name.starts_with('.')
                        && name != "node_modules"
                        && name != "target"
                        && name != "dist"
                })
                .map(|e| {
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    Entry {
                        name: e.file_name().to_string_lossy().to_string(),
                        path: e.path(),
                        is_dir,
                    }
                })
                .collect();

            // Directories first, then alphabetical.
            listing.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });

            entries.extend(listing);
        }

        self.entries = entries;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    /// Returns the currently highlighted entry, if any.
    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Enter the selected directory. No-op for files.
    pub fn enter(&mut self) {
        if let Some(entry) = self.current() {
            if entry.is_dir {
                self.cwd = entry.path.clone();
                self.selected = 0;
                self.refresh();
            }
        }
    }

    /// Path of the current entry relative to the project root, for use as a
    /// chat `file_path`. Returns `None` for directories or the `..` entry.
    pub fn selected_relative_file(&self) -> Option<String> {
        self.current()
            .and_then(|e| self.selected_relative_file_for(e))
    }

    /// Project-relative path for a given entry, or `None` for directories.
    pub fn selected_relative_file_for(&self, entry: &Entry) -> Option<String> {
        if entry.is_dir {
            return None;
        }
        let rel = entry.path.strip_prefix(&self.root).unwrap_or(&entry.path);
        Some(rel.to_string_lossy().to_string())
    }

    pub fn cwd_display(&self) -> String {
        relative_display(&self.root, &self.cwd)
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}
