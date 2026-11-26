//! `winrust-ui` — shared look for the WinRust programs.
//!
//! * [`theme`] — the common dark palette, the egui visuals and the optional
//!   window transparency / blur plumbing.
//!
//! Localisation lives in `winrust_core::i18n` so even non-UI crates (service
//! names, priority labels) can translate their own strings.

pub mod theme;

pub use winrust_core::i18n::Appearance;

/// Where a popup that hangs above a bar lands, given its measured size.
///
/// The taskbar's popups (tray flyouts, the Start menu's own menus, the power
/// menu) all have the same rule: the panel's **bottom edge** sits on the bar's
/// top edge and it grows upward, and its right edge lines up with the button.
/// Doing that needs the panel's final height, which an [`egui::Area`] only
/// knows after it has been laid out once - that is what
/// [`anchored_above`] waits for.
///
/// The old code guessed a height per panel and positioned by the top-left
/// corner, so every guess that was too small pushed the panel's bottom row down
/// behind the bar.  That is the "выпадающие списки съехали" report, and it
/// cannot happen here because no height is ever assumed.
pub fn popup_rect_above(
    size: egui::Vec2,
    button: egui::Rect,
    bar_top: f32,
    screen: egui::Rect,
    gap: f32,
) -> egui::Rect {
    // Right edge on the button, kept inside the screen; the panel may be wider
    // than its button (a flyout is), so clamp the *left* afterwards.
    let right = button.right().min(screen.right() - 8.0);
    let left = (right - size.x).max(screen.left() + 8.0);
    let bottom = bar_top - gap;
    // Growing up past the screen top is the one thing to avoid: an over-tall
    // panel is then pinned to the top edge and simply gets shorter.
    let top = (bottom - size.y).max(screen.top() + 8.0);
    egui::Rect::from_min_size(egui::pos2(left, top), size)
}

/// Show `contents` in an [`egui::Area`] placed by [`popup_rect_above`].
///
/// egui has no "anchor to the bottom edge" primitive for an `Area`, so the size
/// used for placement is the one the area reported on the *previous* frame,
/// cached in egui's own memory under `id`.  On the first frame the panel is
/// shown where the button is (a single frame, invisible at 60 Hz) and from then
/// on its bottom edge rests exactly on the bar.  The alternative - running the
/// contents twice in one frame to measure them - would duplicate every click
/// and every side effect in them.
pub fn anchored_above<R>(
    ctx: &egui::Context,
    id: egui::Id,
    button: egui::Rect,
    bar_top: f32,
    gap: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let screen = ctx.screen_rect();
    let size_id = id.with("__measured_size");
    let cached: Option<egui::Vec2> = ctx.data(|d| d.get_temp(size_id));
    let size = cached.unwrap_or(egui::vec2(0.0, 0.0));
    let rect = if size.x > 0.0 && size.y > 0.0 {
        popup_rect_above(size, button, bar_top, screen, gap)
    } else {
        // First frame: sit on the bar's top edge under the button.  The real
        // placement happens on the next one, once the size is known.
        egui::Rect::from_min_size(
            egui::pos2(button.left().min(screen.right() - 8.0), bar_top - gap),
            egui::vec2(0.0, 0.0),
        )
    };
    let response = egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .fixed_pos(rect.left_top())
        .show(ctx, add_contents);
    let measured = response.response.rect.size();
    if measured.x > 0.0 && measured.y > 0.0 {
        ctx.data_mut(|d| d.insert_temp(size_id, measured));
    }
    response
}
