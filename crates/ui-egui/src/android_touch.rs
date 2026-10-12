//! Android UI ergonomics for PhotoCraft's existing egui interface.
//!
//! eframe's egui-winit backend *already* converts the primary Android touch to
//! pointer events, while preserving native events for multi-touch. Injecting
//! a second PointerButton here makes a single tap paint twice. Leave event
//! conversion to eframe and change only visual/touch target sizing.
#![cfg(target_os = "android")]

use egui::{Context, Theme, vec2};

/// Install 48-point minimum interaction targets after the PhotoCraft theme.
/// This affects widgets honoring egui spacing; explicitly sized custom
/// widgets may need separate hit-area adjustments.
///
/// The OS/winit pipeline reports positions in egui points, so this intentionally
/// does not multiply input coordinates by Android's physical pixel density.
pub fn configure_touch_ui(ctx: &Context) {
    ctx.set_zoom_factor(1.0);
    // Permit small S Pen contact drift without classifying a tap as a drag.
    // egui measures this distance in logical points, not physical pixels.
    ctx.options_mut(|options| {
        options.input_options.max_click_dist =
            options.input_options.max_click_dist.max(24.0);
    });
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
            style.spacing.interact_size.y = style.spacing.interact_size.y.max(48.0);
            style.spacing.interact_size.x = style.spacing.interact_size.x.max(48.0);
            style.spacing.button_padding = vec2(12.0, 10.0);
            style.spacing.item_spacing.x = style.spacing.item_spacing.x.max(8.0);
            style.spacing.item_spacing.y = style.spacing.item_spacing.y.max(8.0);
            // egui's default 3 pt window-edge grab radius is intended for a mouse.
            // Keep these Android-only so desktop hit testing is unchanged.
            style.interaction.interact_radius = style.interaction.interact_radius.max(8.0);
            style.interaction.resize_grab_radius_side = style.interaction.resize_grab_radius_side.max(18.0);
            style.interaction.resize_grab_radius_corner = style.interaction.resize_grab_radius_corner.max(24.0);
        });
    }
}
