//! Clipboard history - the list the shell shows on `Win+V`.
//!
//! Windows' own history is a WinRT feature of `explorer.exe`: when the stock
//! shell is replaced, `Win+V` stops working, which is why the review asked for
//! a history that does not depend on it.  The list itself is ordinary state,
//! so it lives here - in the core, where every component can share one
//! definition - and the desktop owns one instance of it.
//!
//! [`History`] keeps at most [`History::CAPACITY`] entries, most recent first,
//! drops exact duplicates (pasting the same text twice must not fill the list),
//! and never stores empty text.  The UI cannot break those rules because it
//! only has [`History::record`], [`History::entries`] and [`History::pick`].

use std::time::Instant;

/// One remembered clipboard value.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The text itself, possibly truncated for display by the UI.
    pub text: String,
    /// When it was copied, for the "just now" line under the entry.
    pub at: Instant,
}

/// A bounded, de-duplicated clipboard history.
#[derive(Debug, Default)]
pub struct History {
    entries: Vec<Entry>,
    /// The last value [`History::observe`] saw, so a poll that sees the same
    /// clipboard twice does not record it twice.
    seen: Option<String>,
}

impl History {
    /// How many entries the panel lists.
    pub const CAPACITY: usize = 40;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add `text` as the newest entry.
    ///
    /// Empty text and a repeat of the newest entry are ignored: the clipboard
    /// poll runs several times a second and would otherwise fill the list with
    /// copies of whatever is already there.
    pub fn record(&mut self, text: &str) {
        let text = text.trim_end_matches('\0');
        if text.trim().is_empty() {
            return;
        }
        if self
            .entries
            .first()
            .map(|e| e.text == text)
            .unwrap_or(false)
        {
            return;
        }
        // A value copied again moves to the top instead of appearing twice.
        self.entries.retain(|e| e.text != text);
        self.entries.insert(
            0,
            Entry {
                text: text.to_string(),
                at: Instant::now(),
            },
        );
        self.entries.truncate(Self::CAPACITY);
    }

    /// Poll the system clipboard once and record it when it changed.
    ///
    /// Cheap: `clipboard_text()` is one `OpenClipboard`/`GetClipboardData`
    /// pair, and the string comparison short-circuits on the common case where
    /// nothing was copied since the last poll.
    pub fn observe(&mut self) {
        let Some(text) = crate::shell::clipboard_text() else {
            return;
        };
        if self.seen.as_deref() == Some(text.as_str()) {
            return;
        }
        self.seen = Some(text.clone());
        self.record(&text);
    }

    /// Put entry `index` back on the clipboard.  `true` when Windows took it.
    pub fn pick(&mut self, index: usize) -> bool {
        let Some(entry) = self.entries.get(index) else {
            return false;
        };
        let ok = crate::shell::set_clipboard_text(&entry.text);
        if ok {
            // Picking moves it to the top, like Windows does.
            let text = entry.text.clone();
            self.record(&text);
        }
        ok
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
