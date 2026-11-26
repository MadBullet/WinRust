//! Alt+Tab: the window switcher.
//!
//! Windows keeps this inside `explorer.exe`, so replacing the shell takes the
//! switcher away with it.  This is our own: hold `Alt`, tap `Tab` while it is
//! held, and a panel lists every open window newest-first with its picture;
//! tapping `Tab` walks the selection, `Shift+Tab` walks it back, releasing
//! `Alt` raises the selected window, and a click raises the one clicked.
//!
//! ## Why the keys are polled
//!
//! `Alt+Tab` is swallowed by the system before any window sees it - it is a
//! reserved shell chord, not an ordinary key event - so a `WM_KEYDOWN` handler
//! would never fire.  `GetAsyncKeyState` still reports the physical keys while
//! another program has the focus, which is the same trick `Win+V` and `Win+I`
//! use, and it is the only way to see the chord from a window that is behind
//! everything.
//!
//! ## Why the state is a value, not three fields
//!
//! "Open, selection, and the order it walks" only make sense together: a
//! selection without the panel is nothing, and an order without a selection
//! cannot be walked.  They live in one struct with verbs, so the shell cannot
//! put the panel into a state the drawing code does not expect.

use crate::DesktopApp;

/// Virtual key codes, straight from `winuser.h`.
///
/// Written as numbers on purpose: `VK_MENU` is Alt (either side), `VK_TAB` is
/// Tab, `VK_SHIFT` is Shift (either side) and `VK_ESCAPE` is the Esc key.
/// The values are fixed by Windows and have not changed since 1993.
const VK_TAB: i32 = 0x09;
const VK_MENU: i32 = 0x12;
const VK_SHIFT: i32 = 0x10;
const VK_ESCAPE: i32 = 0x1B;

/// One window the switcher can offer.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub(crate) hwnd: isize,
    pub(crate) pid: u32,
    pub(crate) title: String,
}

impl Candidate {
    /// The key the picture cache uses.  Both the taskbar preview and this
    /// panel show the same window, so they share one texture per window.
    pub(crate) fn cache_key(&self) -> isize {
        self.hwnd
    }
}

/// The switcher's whole state.
#[derive(Debug, Default)]
pub(crate) struct AltTab {
    /// The windows on offer, most recently used first.  Built when the chord
    /// opens, so the order does not shuffle while the user is choosing.
    candidates: Vec<Candidate>,
    /// Which candidate the highlight is on.  The *first* entry is "the window
    /// you are leaving", so a single `Tab` selects entry one - the previous
    /// window - exactly like the stock switcher.
    selected: usize,
    /// `Alt` was still held on the last frame.  The edge is what closes the
    /// panel: releasing `Alt` is the "go" gesture.
    alt_was_down: bool,
    /// `Tab` was still held on the last frame, for the same reason - holding
    /// `Tab` must walk the selection once, not once per frame.
    tab_was_down: bool,
    /// Pictures have been captured for this opening of the panel.
    captured: bool,
    /// Whether the shell's own window is currently raised above everything.
    /// The shell is `always_on_bottom` by design - an ordinary window would
    /// cover the desktop it *is* - so the one panel that must be seen over
    /// other programs raises the window for as long as it is up, and puts it
    /// back afterwards.
    pub(crate) raised: bool,
}

impl AltTab {
    /// True while the panel is showing.
    fn is_open(&self) -> bool {
        !self.candidates.is_empty()
    }

    fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    fn selected(&self) -> usize {
        self.selected
    }

    /// Paint while there is anything to choose between.  One window is
    /// still a valid switcher - it is just a panel with a single card - and
    /// suppressing it would make `Alt+Tab` look broken on a bare desktop.
    pub(crate) fn visible(&self) -> bool {
        !self.candidates.is_empty()
    }

    /// The window that would be raised if `Alt` were released now.
    fn selected_hwnd(&self) -> Option<isize> {
        self.candidates.get(self.selected).map(|c| c.hwnd)
    }

    /// Walk the selection `step` places, wrapping at both ends.
    fn walk(&mut self, step: isize) {
        if self.candidates.is_empty() {
            return;
        }
        let n = self.candidates.len() as isize;
        self.selected = (((self.selected as isize + step) % n + n) % n) as usize;
    }

    /// Drop the panel without raising anything (`Esc`).
    fn close(&mut self) {
        self.candidates.clear();
        self.selected = 0;
        self.captured = false;
        self.raised = false;
    }

    /// One frame of the chord's state machine.  Returns the window to raise
    /// when `Alt` comes up, if any.
    ///
    /// `frame` is the shell's own window: it must never appear in its own
    /// switcher, because it *is* the desktop.
    pub(crate) fn tick(&mut self, frame_hwnd: isize, list: &[crate::taskbar::WindowEntry]) -> Option<isize> {
        let alt = winrust_core::window::key_down(VK_MENU);
        let tab = winrust_core::window::key_down(VK_TAB);
        let shift = winrust_core::window::key_down(VK_SHIFT);
        // `Esc` closes the panel but keeps `Alt` held; the shell's other
        // handlers already act on the edge, so a plain read is enough here.
        let esc = winrust_core::window::key_down(VK_ESCAPE);

        let mut raise = None;

        // Opening: `Alt` held and `Tab` just went down.  `opened` is kept so
        // the same press is not also walked below - otherwise the first card
        // the user ever sees is already the second window.
        let mut opened = false;
        if alt && tab && !self.tab_was_down && !self.is_open() {
            // `EnumWindows` hands windows back in z-order, *oldest first*, so
            // the front-most window is the last one.  The switcher wants the
            // opposite: the window you are leaving sits at index 0, and index
            // 1 is the one you used before it.  Reversing here is what makes
            // "one Tab" mean "go back where I was".
            self.candidates = list
                .iter()
                .filter(|w| w.hwnd != frame_hwnd)
                .rev()
                .map(|w| Candidate {
                    hwnd: w.hwnd,
                    pid: w.pid,
                    title: w.title.clone(),
                })
                .collect();
            // Entry 0 is the window being left.  The highlight starts on it
            // and the `Tab` that opened the panel is *consumed* by the opening
            // itself - `tab_was_down` is updated at the end of this tick, so
            // the next tap walks to entry 1.  That makes the first visible
            // move land on the previous window, which is the stock behaviour.
            self.selected = 0;
            self.captured = false;
            opened = true;
        }

        if self.is_open() {
            // Holding `Tab` must not sweep through the list at the frame rate,
            // and the press that opened the panel must not count as a step.
            if tab && !self.tab_was_down && !opened {
                self.walk(if shift { -1 } else { 1 });
            }
            if esc {
                self.close();
            }
        }

        // Releasing `Alt` is the "go" gesture.
        if !alt && self.alt_was_down && self.is_open() {
            raise = self.selected_hwnd();
            self.close();
        }

        self.alt_was_down = alt;
        self.tab_was_down = tab;
        raise
    }
}

impl DesktopApp {
    /// The switcher's panel: one row of window cards, newest-used first, the
    /// selected one outlined.  Drawn over everything, above the taskbar.
    pub(crate) fn alt_tab_panel(&mut self, ctx: &egui::Context) {
        if !self.alt_tab.visible() {
            return;
        }
        let pal = winrust_ui::theme::pal(&self.appearance);
        let screen = ctx.screen_rect();

        // Pictures are captured once, when the panel opens.  A live refresh
        // was tried and is the wrong trade: `PrintWindow` on every window in
        // the list costs more than the switcher is worth, and the panel is up
        // for a fraction of a second anyway.  A picture is dropped when its
        // window is gone so the cache cannot grow for the life of the shell.
        let missing = self
            .alt_tab
            .candidates()
            .iter()
            .any(|c| !self.thumbs.contains_key(&c.cache_key()));
        if !self.alt_tab.captured || missing {
            self.alt_tab.captured = true;
            let live: std::collections::HashSet<isize> = self
                .alt_tab
                .candidates()
                .iter()
                .map(|c| c.cache_key())
                .collect();
            self.thumbs.retain(|hwnd, _| live.contains(hwnd));
            for c in self.alt_tab.candidates().to_vec() {
                let image = winrust_core::thumb::window_thumbnail(c.hwnd, 320, 200).map(|t| {
                    egui::ColorImage::from_rgba_unmultiplied(
                        [t.width as usize, t.height as usize],
                        &t.rgba,
                    )
                });
                let handle = image
                    .map(|img| ctx.load_texture("alt_tab", img, egui::TextureOptions::LINEAR));
                self.thumbs.insert(c.cache_key(), handle);
            }
        }

        // Card size follows the screen, so the strip fits on a laptop as well
        // as on a desktop: aim for a card about 1/5 of the width.
        let card_w = (screen.width() / 5.5).clamp(150.0, 260.0);
        let card_h = card_w * 0.62;
        let gap = 14.0;
        let pad = 22.0;
        let count = self.alt_tab.candidates().len() as f32;
        // Wrap into rows, like the stock switcher does past five windows.
        let per_row = 5usize;
        let rows = self.alt_tab.candidates().len().div_ceil(per_row) as f32;
        let row_count = self.alt_tab.candidates().len().min(per_row) as f32;
        let width = row_count * card_w + (row_count - 1.0) * gap + pad * 2.0;
        let height = rows * card_h + (rows - 1.0) * gap + pad * 2.0 + 26.0;
        let rect = egui::Rect::from_center_size(screen.center(), egui::vec2(width, height));

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("alt_tab_panel"),
        ));
        painter.rect_filled(rect, 12.0, panel_surface(pal));
        painter.rect_stroke(rect, 12.0, egui::Stroke::new(1.0_f32, panel_line(pal)));

        let selected = self.alt_tab.selected();
        for (i, c) in self.alt_tab.candidates().iter().enumerate() {
            let row = i / per_row;
            let col = i % per_row;
            let card = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + pad + col as f32 * (card_w + gap),
                    rect.top() + pad + row as f32 * (card_h + gap),
                ),
                egui::vec2(card_w, card_h),
            );
            let tex = self.thumbs.get(&c.cache_key()).and_then(|t| t.as_ref());
            match tex {
                Some(t) => {
                    let size = t.size_vec2();
                    let scale = (card.width() / size.x).min(card.height() / size.y);
                    let draw = egui::Rect::from_center_size(card.center(), size * scale);
                    painter.image(
                        t.id(),
                        draw,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    painter.rect_filled(card, 6.0, panel_card(pal));
                }
            }
            painter.text(
                egui::pos2(card.center().x, card.top() - 2.0),
                egui::Align2::CENTER_BOTTOM,
                winrust_core::i18n::end(&c.title, 26),
                egui::FontId::proportional(12.0),
                panel_text(pal, i == selected),
            );
            if i == selected {
                painter.rect_stroke(
                    card.expand(3.0),
                    8.0,
                    egui::Stroke::new(2.5_f32, panel_accent(pal)),
                );
            }
        }
        // The hint line, the way the stock switcher names the gesture.
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - 12.0),
            egui::Align2::CENTER_CENTER,
            winrust_core::t("Release Alt to switch"),
            egui::FontId::proportional(11.0),
            panel_weak(pal),
        );

        // A click on a card switches to it directly - the "либо выбрать"
        // half of the request.  Registered as one area over the whole panel,
        // so it cannot be swallowed by the desktop behind it.
        let clicked = egui::Area::new(egui::Id::new("alt_tab_hit"))
            .order(egui::Order::Tooltip)
            .fixed_pos(rect.left_top())
            .show(ctx, |ui| {
                let resp = ui.allocate_exact_size(rect.size(), egui::Sense::click()).1;
                resp.clicked().then(|| ui.input(|i| i.pointer.interact_pos()))
            })
            .inner
            .flatten();
        if let Some(p) = clicked {
            for (i, _) in self.alt_tab.candidates().iter().enumerate() {
                let row = i / per_row;
                let col = i % per_row;
                let card = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + pad + col as f32 * (card_w + gap),
                        rect.top() + pad + row as f32 * (card_h + gap),
                    ),
                    egui::vec2(card_w, card_h),
                );
                if card.expand(3.0).contains(p) {
                    if let Some(hwnd) = self.alt_tab.candidates().get(i).map(|c| c.hwnd) {
                        winrust_core::window::activate_window_hwnd(hwnd);
                        self.alt_tab.close();
                    }
                    break;
                }
            }
        }
    }
}

/// The panel's colours, read from the palette in exactly one place.
///
/// Repeating the three `.0/.1/.2` accesses at every call site is how a
/// copy-paste slip hides - one of them ends up on `surface` where the rest
/// meant `tile`.  One function per colour keeps that impossible.
fn panel_surface(pal: winrust_ui::theme::Palette) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(pal.surface.0, pal.surface.1, pal.surface.2, 250)
}

fn panel_line(pal: winrust_ui::theme::Palette) -> egui::Color32 {
    egui::Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2)
}

fn panel_card(pal: winrust_ui::theme::Palette) -> egui::Color32 {
    egui::Color32::from_rgb(pal.tile.0, pal.tile.1, pal.tile.2)
}

fn panel_accent(pal: winrust_ui::theme::Palette) -> egui::Color32 {
    egui::Color32::from_rgb(pal.accent.0, pal.accent.1, pal.accent.2)
}

fn panel_text(pal: winrust_ui::theme::Palette, strong: bool) -> egui::Color32 {
    let c = if strong { pal.text } else { pal.text_weak };
    egui::Color32::from_rgb(c.0, c.1, c.2)
}

fn panel_weak(pal: winrust_ui::theme::Palette) -> egui::Color32 {
    egui::Color32::from_rgb(pal.text_weak.0, pal.text_weak.1, pal.text_weak.2)
}
