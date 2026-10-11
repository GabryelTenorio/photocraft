//! Embedded preset selection, sharing the sidebar's folders, grid and management commands.

use super::*;
use photocraft_engine::presets::gradients::{GradientPreset, StopColor};

fn target(app: &PhotocraftApp) -> Sink {
    if !app.ui.tool_options.gradient_classic
        && let Some(layer) = selected_gradient(app)
    {
        Sink::Layer(layer.id)
    } else {
        Sink::Preset
    }
}

fn new_params(app: &PhotocraftApp, sink: Sink) -> Value {
    if let Sink::Layer(_) = sink
        && let Some(Fill::Gradient { stops, opacity_stops, .. }) = sink.fill(app)
    {
        let stops = stops
            .into_iter()
            .map(|(location, color)| {
                let rgb = color.to_rgb();
                (location, StopColor::Rgb([rgb[0], rgb[1], rgb[2]]))
            })
            .collect();
        GradientPreset { name: "Custom".into(), stops, opacity: opacity_stops }.to_json()
    } else {
        json!({"name": "Custom"})
    }
}

pub(super) fn show(app: &mut PhotocraftApp, ui: &mut egui::Ui, panel: &str) -> bool {
    let sink = target(app);
    show_for(app, ui, panel, sink)
}

fn show_for(app: &mut PhotocraftApp, ui: &mut egui::Ui, panel: &str, sink: Sink) -> bool {
    let params = new_params(app, sink);
    if !crate::preset_panels::gradient_picker(app, ui, panel, matches!(sink, Sink::Layer(_)), params) {
        return false;
    }
    let key = strip_key(app, sink).with("selection");
    ui.data_mut(|d| {
        d.remove::<Marker>(strip_key(app, sink));
        let generation = d.get_temp::<u64>(key.with("generation")).unwrap_or_default();
        d.insert_temp(key.with("generation"), generation.wrapping_add(1));
        d.insert_temp(key, Marker::Color(0));
    });
    match sink {
        Sink::Layer(id) => {
            if app.gradient.panel.as_ref().is_some_and(|(layer, ..)| *layer == id) {
                app.gradient.panel = None;
            }
        }
        Sink::Preset => app.gradient.preset_stop = None,
    }
    true
}

pub(super) fn layer_picker(app: &mut PhotocraftApp, ui: &mut egui::Ui, sink: Sink) {
    let arrow = crate::icons::button(ui, "chevron-down", 20.0, false, "Presets");
    arrow.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tl!("Presets")));
    egui::Popup::from_toggle_button_response(&arrow).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_width(320.0);
        if show_for(app, ui, "gradient-properties-presets", sink) {
            ui.close();
        }
    });
}

pub(super) fn swatch(app: &mut PhotocraftApp, ui: &mut egui::Ui) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let t = Tokens::get(ui.ctx());
        let classic = app.ui.tool_options.gradient_classic;
        let stops = app.session.presets.gradient.resolve(app.session.tools.foreground, app.session.tools.background);
        let (r, resp) = ui.allocate_exact_size(vec2(96.0, 20.0), if classic { Sense::click() } else { Sense::hover() });
        paint_ramp(ui.painter(), r, |u| photocraft_algo::paint::sample_stops(&stops, u));
        ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, t.field_border), StrokeKind::Outside);
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                if classic { egui::WidgetType::Button } else { egui::WidgetType::Label },
                true,
                if classic { tl!("Gradient picker") } else { tl!("Gradient") },
            )
        });
        let open_editor = if classic { resp.on_hover_text(tl!("Click to edit the gradient")).clicked() } else { false };
        let arrow = crate::icons::button(ui, "chevron-down", 20.0, false, "Presets");
        arrow.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tl!("Presets")));
        egui::Popup::from_toggle_button_response(&arrow).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_width(320.0);
            if show(app, ui, "gradient-picker") {
                ui.close();
            }
        });
        open_editor
    })
    .inner
}
