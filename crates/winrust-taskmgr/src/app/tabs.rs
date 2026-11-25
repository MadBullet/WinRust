//! The tab strip down the left side, and what each tab is called.

use winrust_core::ts;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Processes,
    Details,
    Performance,
    Startup,
    Users,
    Network,
    Services,
    Settings,
}

impl Tab {
    pub(super) fn label(self) -> &'static str {
        match self {
            Tab::Processes => ts("Processes"),
            Tab::Details => ts("Details"),
            Tab::Performance => ts("Performance"),
            Tab::Startup => ts("Startup"),
            Tab::Users => ts("Users"),
            Tab::Network => ts("Network"),
            Tab::Services => ts("Services"),
            Tab::Settings => ts("Settings"),
        }
    }
    pub(super) fn glyph(self) -> &'static str {
        match self {
            Tab::Processes => "▤",
            Tab::Details => "☷",
            Tab::Performance => "📈",
            Tab::Startup => "🚀",
            Tab::Users => "👤",
            Tab::Network => "🌐",
            Tab::Services => "⚙",
            Tab::Settings => "☰",
        }
    }
    const ALL: [Tab; 8] = [
        Tab::Processes,
        Tab::Details,
        Tab::Performance,
        Tab::Startup,
        Tab::Users,
        Tab::Network,
        Tab::Services,
        Tab::Settings,
    ];
}
