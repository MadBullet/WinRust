//! File verbs - open, copy, cut, paste, delete, rename, new folder - shared
//! by the context menus and the keyboard, so both always agree.

use super::state::{ExplorerApp, FileClipboard, JobUpdate};
use super::tabs::{file_name_of, FileJob, Tab};
use eframe::egui;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use winrust_core::disk;
use winrust_core::shell;
use winrust_core::shell::KnownFolder;

// Not `Copy`: `OpenWithProgram` carries a command line and a path, and every
// use site either moves or matches by reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Command {
    Open,
    OpenNewTab,
    Edit,
    OpenWith,
    /// Run one registered program on one file (`Open with`).
    OpenWithProgram(String, PathBuf),
    RunAsAdmin,
    Copy,
    Cut,
    Paste,
    Rename,
    Delete,
    DeletePermanent,
    CopyPath,
    Properties,
    SendToDesktop,
    ExtractHere,
    SelectAll,
    InvertSelection,
    Refresh,
}

impl ExplorerApp {
    pub(super) fn run(&mut self, path: PathBuf, cmd: Command) {
        match cmd {
            Command::Open => {
                if path.is_dir() {
                    self.navigate(path);
                } else if let Err(e) = shell::open(&path) {
                    self.status = format!("Open failed: {e}");
                }
            }
            Command::OpenNewTab => {
                if path.is_dir() {
                    self.tabs.push(Tab::new(path.clone()));
                    let idx = self.tabs.len() - 1;
                    self.active = idx;
                    self.start_load(idx);
                }
            }
            Command::Edit => {
                if let Err(e) = shell::open_verb(&path, "edit") {
                    self.status = format!("Edit failed: {e}");
                }
            }
            Command::OpenWith => {
                let args = format!("shell32.dll,OpenAs_RunDLL {}", path.display());
                if let Err(e) = shell::launch("rundll32.exe", &args) {
                    self.status = format!("Open with failed: {e}");
                }
            }
            Command::OpenWithProgram(command, target) => {
                // The registry command is `"C:\...\app.exe" "%1"`.
                match winrust_core::shell::run_command_for_file(&command, &target) {
                    Ok(()) => self.status = format!("{}: {}", command, target.display()),
                    Err(e) => self.status = format!("Open with failed: {e}"),
                }
            }
            Command::RunAsAdmin => {
                if let Err(e) = shell::run_as_admin(&path) {
                    self.status = format!("Run as admin failed: {e}");
                }
            }
            Command::Copy => {
                self.clipboard = Some(FileClipboard {
                    paths: vec![path.clone()],
                    cut: false,
                });
                self.status = format!("Copied: {}", file_name_of(&path));
            }
            Command::Cut => {
                self.clipboard = Some(FileClipboard {
                    paths: vec![path.clone()],
                    cut: true,
                });
                self.status = format!("Cut: {}", file_name_of(&path));
            }
            Command::Paste => self.paste_clipboard(),
            Command::Rename => {
                self.rename_buffer = file_name_of(&path);
                self.rename_target = Some(path);
            }
            Command::Delete => {
                self.delete_confirm = vec![path];
                self.delete_permanent = false;
            }
            Command::DeletePermanent => {
                self.delete_confirm = vec![path];
                self.delete_permanent = true;
            }
            Command::CopyPath => {
                let text = path.display().to_string();
                if shell::set_clipboard_text(&text) {
                    self.status = format!("Path copied: {text}");
                } else {
                    self.status = text;
                }
            }
            Command::Properties => {
                if let Err(e) = shell::show_properties(&path) {
                    self.status = format!("Properties failed: {e}");
                }
            }
            Command::SendToDesktop => {
                if let Some(desktop) = KnownFolder::Desktop.path() {
                    let name = file_name_of(&path);
                    let dest = disk::unique_destination(desktop.join(name));
                    match disk::copy_tree(&path, &dest, true) {
                        Ok(_) => self.status = format!("Shortcut copy → {}", dest.display()),
                        Err(e) => self.status = format!("Send to Desktop failed: {e}"),
                    }
                }
            }
            Command::ExtractHere => {
                // Explorer's "Extract All" for archives; Windows' built-in
                // zipfldr handler understands this verb.
                if let Err(e) = shell::open_verb(&path, "extract") {
                    self.status = format!("Extract failed: {e}");
                }
            }
            Command::SelectAll => {
                let entries = self.sorted_filtered();
                self.current_mut().selection = entries.into_iter().map(|e| e.path).collect();
                self.current_mut().anchor = None;
            }
            Command::InvertSelection => {
                let entries = self.sorted_filtered();
                let existing: HashSet<PathBuf> = self.current().selection.iter().cloned().collect();
                self.current_mut().selection = entries
                    .into_iter()
                    .map(|e| e.path)
                    .filter(|p| !existing.contains(p))
                    .collect();
            }
            Command::Refresh => self.refresh(),
        }
    }

    pub(super) fn run_selection(&mut self, cmd: Command) {
        let sel: Vec<PathBuf> = self.current().selection.clone();
        if sel.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        // Matched by reference so the two arms that forward `cmd` can still use
        // it; `Command` is no longer `Copy` (one variant owns a String).
        match &cmd {
            Command::Paste => self.paste_clipboard(),
            Command::SelectAll => self.run(PathBuf::new(), cmd),
            Command::InvertSelection => self.run(PathBuf::new(), cmd),
            Command::Refresh => self.refresh(),
            Command::Copy => {
                self.clipboard = Some(FileClipboard {
                    paths: sel.clone(),
                    cut: false,
                });
                self.status = format!("Copied {} item(s)", sel.len());
            }
            Command::Cut => {
                self.clipboard = Some(FileClipboard {
                    paths: sel.clone(),
                    cut: true,
                });
                self.status = format!("Cut {} item(s)", sel.len());
            }
            Command::Delete => {
                self.delete_confirm = sel;
                self.delete_permanent = false;
            }
            Command::DeletePermanent => {
                self.delete_confirm = sel;
                self.delete_permanent = true;
            }
            other => {
                // `other` is a reference now, and each iteration needs an owned
                // Command (one variant carries a String and a PathBuf).
                let owned = other.clone();
                for p in sel {
                    self.run(p.clone(), owned.clone());
                }
            }
        }
    }

    /// Ctrl+V — copy or move the staged entries into the current folder.
    /// Existing names get an Explorer-style `name (2).ext` suffix.
    pub(super) fn paste_clipboard(&mut self) {
        let Some(clip) = self.clipboard.take() else {
            self.status = "Clipboard is empty".into();
            return;
        };
        let dest_dir = self.current().path.clone();
        if clip.paths.iter().any(|src| src == &dest_dir) {
            self.status = "Cannot paste a folder into itself".into();
            self.clipboard = Some(clip);
            return;
        }
        let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
        for src in &clip.paths {
            let Some(name) = src.file_name() else {
                continue;
            };
            let mut dest = dest_dir.join(name);
            if dest.exists() {
                dest = disk::unique_destination(&dest);
            }
            jobs.push((src.clone(), dest));
        }
        self.start_file_job(jobs, clip.cut, self.active);
    }

    /// Spawn a worker that performs `copy_tree` / `move_path` for every pair and
    /// streams progress back to the UI.
    pub(super) fn start_file_job(&mut self, jobs: Vec<(PathBuf, PathBuf)>, cut: bool, tab: usize) {
        if jobs.is_empty() {
            return;
        }
        let verb = if cut { "Moving" } else { "Copying" };
        let label = format!("{verb} {} item(s)", jobs.len());
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(FileJob {
            label,
            done: 0,
            total: jobs.len(),
            errors: Vec::new(),
            cancel: cancel.clone(),
            finished: false,
            refresh_tab: tab,
        });
        let tx = self.job_tx.clone();
        std::thread::spawn(move || {
            let mut done = 0usize;
            let mut errors = Vec::new();
            for (src, dest) in jobs {
                if cancel.load(Ordering::Relaxed) {
                    errors.push("Cancelled".to_string());
                    break;
                }
                let result = if cut {
                    disk::move_path(&src, &dest, false)
                } else {
                    disk::copy_tree(&src, &dest, true).map(|_| ())
                };
                match result {
                    Ok(()) => done += 1,
                    Err(e) => errors.push(format!("{}: {e}", file_name_of(&src))),
                }
                let _ = tx.send(JobUpdate {
                    done,
                    errors: errors.clone(),
                    finished: false,
                });
            }
            let _ = tx.send(JobUpdate {
                done,
                errors,
                finished: true,
            });
        });
    }

    pub(super) fn pump_workers(&mut self, ctx: &egui::Context) {
        // Directory listings
        while let Ok(listing) = self.rx.try_recv() {
            if let Some(tab) = self.tabs.get_mut(listing.tab_index) {
                if tab.request_id == listing.request_id {
                    tab.loading = false;
                    match listing.result {
                        Ok(mut entries) => {
                            // Fill the localized type text once per extension.
                            for e in &mut entries {
                                let key = if e.is_dir {
                                    "dir".to_string()
                                } else {
                                    e.ext.clone()
                                };
                                if let Some(t) = self.type_names.get(&key) {
                                    e.type_name = t.clone();
                                } else {
                                    let t = shell::type_name_for_extension(&e.ext, e.is_dir);
                                    self.type_names.insert(key, t.clone());
                                    e.type_name = t;
                                }
                            }
                            tab.entries = entries;
                            tab.error = None;
                        }
                        Err(e) => {
                            tab.entries.clear();
                            tab.error = Some(e);
                        }
                    }
                    if tab.scroll_to_top {
                        tab.scroll_to_top = false;
                    }
                }
            }
        }

        // Folder aggregate sizes
        while let Ok((path, bytes, count)) = self.folder_size_rx.try_recv() {
            if self.folder_size_pending.as_ref() == Some(&path) {
                self.folder_size = Some((path.clone(), bytes, count));
                self.folder_size_pending = None;
            }
        }

        // File jobs
        while let Ok(update) = self.job_rx.try_recv() {
            if let Some(job) = &mut self.job {
                job.done = update.done;
                job.errors = update.errors;
                job.finished = update.finished;
                if update.finished {
                    let errors = job.errors.clone();
                    let done = job.done;
                    let refresh_tab = job.refresh_tab;
                    self.job = None;
                    self.status = if errors.is_empty() {
                        format!("Done: {done} item(s)")
                    } else {
                        format!(
                            "{done} done, {} error(s): {}",
                            errors.len(),
                            errors.join("; ")
                        )
                    };
                    if self.tabs.get(refresh_tab).is_some() {
                        self.start_load(refresh_tab);
                    }
                    // A successful cut consumes the clipboard.
                    if errors.is_empty() {
                        if let Some(c) = &mut self.clipboard {
                            if c.cut {
                                self.clipboard = None;
                            }
                        }
                    }
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint();
                }
            }
        }

        // Search results
        while let Ok((path, results)) = self.search_rx.try_recv() {
            if self.search_path == path {
                self.search_results = Some(results);
            }
        }
    }

    /// Ctrl+F / typing into the search box: match names in the current folder
    /// *and* its subfolders, like Explorer's search box.
    pub(super) fn start_search(&mut self, needle: String) {
        self.current_mut().filter = needle.clone();
        if needle.is_empty() {
            self.search_results = None;
            return;
        }
        if self.last_search.elapsed().as_millis() < 150 {
            return;
        }
        self.last_search = std::time::Instant::now();
        let root = self.current().path.clone();
        self.search_path = root.clone();
        let tx = self.search_tx.clone();
        let show_hidden = self.current().show_hidden;
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut stack = vec![root.clone()];
            let mut budget = 40_000u32;
            let needle_l = needle.to_lowercase();
            while let Some(dir) = stack.pop() {
                if budget == 0 {
                    break;
                }
                let Ok(entries) = disk::read_dir(&dir, show_hidden) else {
                    continue;
                };
                for e in entries {
                    budget = budget.saturating_sub(1);
                    if budget == 0 {
                        break;
                    }
                    if e.name.to_lowercase().contains(&needle_l) {
                        out.push(e.clone());
                    }
                    if e.is_dir && !e.reparse {
                        stack.push(e.path.clone());
                    }
                }
                if out.len() > 2000 {
                    break;
                }
            }
            let _ = tx.send((root, out));
        });
    }
}
