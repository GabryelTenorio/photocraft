//! #4092 baseline: normal immediate viewport callbacks and separate RawInput streams.
//! This documented egui integration creates no OS windows; actual wgpu renders all frames.
use super::*;
use egui_kittest::{TestRenderer, wgpu::WgpuTestRenderer};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
type Harness = egui_kittest::Harness<'static, PhotocraftApp>;
type Renderer = Rc<RefCell<Option<WgpuTestRenderer>>>;
#[derive(Default)]
struct Backend {
    events: BTreeMap<egui::ViewportId, Vec<egui::Event>>,
    close: BTreeSet<egui::ViewportId>,
    output: BTreeMap<egui::ViewportId, egui::FullOutput>,
    limit: usize,
    passes: usize,
    focused: egui::ViewportId,
    viewports: BTreeMap<egui::ViewportId, egui::ViewportInfo>,
    errors: Vec<String>,
}
type BackendRef = Rc<RefCell<Backend>>;

// Unregister while wgpu's own TLS is still alive, including normal test-assertion unwinding.
// egui stores this callback in thread-local storage; leaving resources there aborts at TLS teardown.
struct ImmediateIntegration;
impl Drop for ImmediateIntegration {
    fn drop(&mut self) {
        egui::Context::set_immediate_viewport_renderer(|_, _| {});
    }
}
fn cases() -> impl Iterator<Item = (theme::ThemeKind, f32)> {
    [theme::ThemeKind::Pro, theme::ThemeKind::Studio].into_iter().flat_map(|theme| [1.0, 2.0].into_iter().map(move |ppp| (theme, ppp)))
}
fn harness(ppp: f32, theme: theme::ThemeKind) -> (Harness, BackendRef, Renderer, ImmediateIntegration) {
    let backend: BackendRef = Rc::default();
    let renderer: Renderer = Rc::default();
    let (input, child_renderer) = (backend.clone(), renderer.clone());
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let Some(size) = viewport.builder.inner_size else {
            input.borrow_mut().errors.push("document viewport builder omitted its size".into());
            return;
        };
        let (events, close, limit, focused, viewports) = {
            let mut b = input.borrow_mut();
            b.passes += 1;
            if size != egui::vec2(800.0, 600.0) {
                b.errors.push(format!("unexpected actual viewport size {size:?}"));
            }
            let child = b.viewports.entry(viewport.ids.this).or_default();
            child.parent = Some(viewport.ids.parent);
            child.native_pixels_per_point = Some(ppp);
            let focused = b.focused;
            for (id, info) in &mut b.viewports {
                info.focused = Some(*id == focused);
            }
            (
                b.events.remove(&viewport.ids.this).unwrap_or_default(),
                b.close.remove(&viewport.ids.this),
                b.limit,
                b.focused == viewport.ids.this,
                b.viewports.iter().map(|(id, info)| (*id, info.clone())).collect(),
            )
        };
        let mut raw = egui::RawInput {
            viewport_id: viewport.ids.this,
            viewports,
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            max_texture_side: Some(limit),
            events,
            focused,
            time: Some(ctx.input(|i| i.time)),
            ..Default::default()
        };
        if close {
            raw.viewports.entry(viewport.ids.this).or_default().events.push(egui::ViewportEvent::Close);
        }
        let mut output = ctx.run_ui(raw, |ui| (viewport.viewport_ui_cb)(ui));
        if output.pixels_per_point != ppp {
            input.borrow_mut().errors.push(format!("actual child ppp {} != {ppp}", output.pixels_per_point));
        }
        if let Some(renderer) = child_renderer.borrow_mut().as_mut() {
            renderer.handle_delta(&mut output.textures_delta);
        } else {
            input.borrow_mut().errors.push("selected renderer unavailable".into());
        }
        input.borrow_mut().output.insert(viewport.ids.this, output);
    });
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_pixels_per_point(ppp)
        .with_max_steps(64)
        .wgpu_setup(gpu_canvas::wgpu_setup())
        .build_eframe(|cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, theme);
            cc.egui_ctx.set_embed_viewports(false);
            let rs = cc.wgpu_render_state.as_ref().expect("actual selected wgpu device");
            backend.borrow_mut().limit = rs.device.limits().max_texture_dimension_2d as usize;
            eprintln!(
                "4092 adapter={} backend={:?} adapter_limit={} device_limit={} ppp={ppp}",
                rs.adapter.get_info().name,
                rs.adapter.get_info().backend,
                rs.adapter.limits().max_texture_dimension_2d,
                backend.borrow().limit
            );
            // The same fresh renderer state, before the root harness's first texture upload.
            *renderer.borrow_mut() = Some(WgpuTestRenderer::from_render_state(rs.clone()));
            let services = Services {
                export: Some(Box::new(|doc, path, _| {
                    photocraft_io::export(doc, path, &Default::default()).map(|r| (r.bytes, r.warnings)).map_err(|e| e.to_string())
                })),
                write: Some(Box::new(|_, _| Ok(()))),
                ..Default::default()
            };
            let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), services);
            app.run("prefs.set", serde_json::json!({"path":"interface.theme","value":theme.id()})).unwrap();
            app.ui.theme = theme;
            app.set_wgpu(rs.clone());
            app
        });
    focus(&mut h, &backend, egui::ViewportId::ROOT, ppp);
    h.input_mut().max_texture_side = Some(backend.borrow().limit);
    h.run_steps(4);
    (h, backend, renderer, ImmediateIntegration)
}
fn focus(h: &mut Harness, b: &BackendRef, focused: egui::ViewportId, ppp: f32) {
    let mut b = b.borrow_mut();
    b.focused = focused;
    b.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
    for window in &h.state().ui.windows {
        let info = b.viewports.entry(egui::ViewportId::from_hash_of(("docwin", window.id))).or_default();
        info.parent = Some(egui::ViewportId::ROOT);
        info.native_pixels_per_point = Some(ppp);
    }
    for (id, info) in &mut b.viewports {
        info.focused = Some(*id == focused);
    }
    h.input_mut().focused = focused == egui::ViewportId::ROOT;
    h.input_mut().viewports = b.viewports.iter().map(|(id, info)| (*id, info.clone())).collect();
}
fn documents(h: &mut Harness, b: &BackendRef, tabs: bool, ppp: f32) {
    h.state_mut().session.edit_prefs(|p| p.workspace.open_documents_as_tabs = tabs);
    for name in ["Document A", "Document B"] {
        h.state_mut().run("file.new", serde_json::json!({"name":name,"width":256,"height":256,"background":"white"})).unwrap();
    }
    for w in &mut h.state_mut().ui.windows {
        w.view.center = [128.0, 128.0];
        w.view.zoom = ppp;
        w.view.doc_size = [256, 256];
        w.view.fit_pending = false;
    }
    // Primary camera is canonical; preserve exactly the baseline's native child geometry.
    for window in h.state().ui.windows.clone() {
        if window.primary {
            h.state_mut().ui.views[window.document] = window.view;
        }
    }
    focus(h, b, if tabs { egui::ViewportId::ROOT } else { viewport(h, 1) }, ppp);
    h.run_steps(4);
}
fn invoke(h: &mut Harness, id: &str, params: serde_json::Value) {
    let ctx = h.ctx.clone();
    menus::invoke(h.state_mut(), &ctx, id, params).unwrap();
    h.run_steps(3);
}
fn viewport(h: &Harness, doc: usize) -> egui::ViewportId {
    let w = h.state().ui.windows.iter().find(|w| w.document == doc).expect("document window");
    egui::ViewportId::from_hash_of(("docwin", w.id))
}
fn shown_tabs(h: &Harness) -> Vec<usize> {
    h.state().tab_strip.as_ref().map_or_else(Vec::new, |s| s.tabs.iter().map(|(i, _)| *i).collect())
}
fn capture(h: &mut Harness, b: &BackendRef, r: &Renderer, name: &str) {
    // Backend observations are asserted only after both root and nested UI passes have returned.
    assert!(b.borrow().errors.is_empty(), "{:?}", b.borrow().errors);
    assert_eq!(b.borrow().viewports.values().filter(|v| v.focused == Some(true)).count(), 1, "one authoritative focused viewport");
    let name = format!("{}-{name}", h.state().ui.theme.id());
    let Some(dir) = std::env::var_os("PHOTOCRAFT_FLOATING_DOCUMENT_EVIDENCE_DIR") else { return };
    let dir = std::path::Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    h.render().expect("actual root frame").save(dir.join(format!("{name}-root.png"))).unwrap();
    // Restored root geometry equals the actual child's 800x600/ppp geometry in this matrix.
    for (id, out) in &b.borrow().output {
        r.borrow_mut().as_mut().unwrap().render(&h.ctx, out).expect("actual child frame").save(dir.join(format!("{name}-child-{id:?}.png"))).unwrap();
    }
    eprintln!(
        "4092 {name}: tabs={:?} windows={:?} active={:?} child_passes={}",
        shown_tabs(h),
        h.state().ui.windows.iter().map(|w| (w.id, w.document)).collect::<Vec<_>>(),
        h.state().session.active_index(),
        b.borrow().passes
    );
}

#[test]
fn preference_floats_documents_without_duplicate_tabs() {
    let mut failures = Vec::new();
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        assert_eq!(h.state().ui.windows.len(), 2, "existing window creation control");
        assert!(b.borrow().output.contains_key(&viewport(&h, 0)) && b.borrow().output.contains_key(&viewport(&h, 1)), "both actual immediate callbacks ran");
        capture(&mut h, &b, &r, &format!("preference-{ppp}x"));
        if !shown_tabs(&h).is_empty() {
            failures.push(format!("{ppp}x primary floats still appear in tabs: {:?}", shown_tabs(&h)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn floating_primary_pointer_edits_its_document_once() {
    let mut failures = Vec::new();
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        h.state_mut().ui.tool = state::Tool::Pencil;
        h.state_mut().run("tools.setColors", serde_json::json!({"foreground":[1.0,0.0,0.0,1.0]})).unwrap();
        h.state_mut().run("tools.setBrush", serde_json::json!({"brush":{"size":6.0,"pressureSize":false,"smoothing":{"amount":0.0}}})).unwrap();
        h.run_steps(2);
        let id = viewport(&h, 0);
        focus(&mut h, &b, id, ppp);
        let untouched = h.state().session.documents()[1].doc.clone();
        let past: Vec<_> = h.state().session.documents().iter().map(|d| d.history.past_len()).collect();
        capture(&mut h, &b, &r, &format!("pointer-before-{ppp}x"));
        for events in [
            vec![
                egui::Event::PointerMoved(egui::pos2(350.0, 250.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(350.0, 250.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            vec![egui::Event::PointerMoved(egui::pos2(375.0, 250.0))],
            vec![egui::Event::PointerButton {
                pos: egui::pos2(375.0, 250.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ] {
            b.borrow_mut().events.insert(id, events);
            h.step();
        }
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("pointer-after-{ppp}x"));
        let docs = h.state().session.documents();
        assert_eq!(docs[1].doc, untouched, "other document remains byte-identical");
        assert_eq!(docs[1].history.past_len(), past[1]);
        let pixel = docs[0].doc.layers[0].surface().unwrap().rgba(90, 78);
        let one_edit = docs[0].history.past_len() == past[0] + 1;
        eprintln!(
            "4092 pointer-{ppp}x active={:?} native_pixel={pixel:?} initial_past={past:?} final_past={:?}",
            h.state().session.active_index(),
            docs.iter().map(|d| d.history.past_len()).collect::<Vec<_>>()
        );
        if h.state().session.active_index() != Some(0) || pixel != [1.0, 0.0, 0.0, 1.0] || !one_edit {
            failures.push(format!("{ppp}x child pointer failed to activate/paint its primary document: pixel={pixel:?},one_edit={one_edit}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn tabbed_documents_and_explicit_extra_view_keep_their_tabs() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, true, ppp);
        assert_eq!(shown_tabs(&h), [0, 1]);
        assert!(h.state().ui.windows.is_empty());
        let docs: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        invoke(&mut h, "window.newWindowForDocument", serde_json::json!({}));
        assert_eq!(shown_tabs(&h), [0, 1], "additional view retains original primary tab");
        assert_eq!(h.state().ui.windows.len(), 1);
        capture(&mut h, &b, &r, &format!("extra-view-{ppp}x"));
        invoke(&mut h, "window.arrange.consolidateAllToTabs", serde_json::json!({}));
        assert_eq!(shown_tabs(&h), [0, 1]);
        assert!(h.state().ui.windows.is_empty());
        assert_eq!(
            h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(),
            docs,
            "view commands preserve model/history"
        );
    }
}

#[test]
fn closing_an_explicit_extra_view_keeps_the_dirty_document() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, true, ppp);
        h.state_mut().run("layer.new.layer", serde_json::json!({})).unwrap();
        let before = h.state().session.active().unwrap().doc.clone();
        assert!(h.state().session.active().unwrap().is_dirty());
        invoke(&mut h, "window.newWindowForDocument", serde_json::json!({}));
        let id = viewport(&h, 1);
        focus(&mut h, &b, id, ppp);
        capture(&mut h, &b, &r, &format!("extra-close-before-{ppp}x"));
        b.borrow_mut().close.insert(id);
        h.step();
        h.run_steps(3);
        focus(&mut h, &b, egui::ViewportId::ROOT, ppp);
        assert!(h.state().ui.windows.is_empty());
        assert!(h.state().discard.is_none());
        assert_eq!(h.state().session.active().unwrap().doc, before);
        assert_eq!(shown_tabs(&h), [0, 1]);
    }
}

#[test]
fn closing_a_clean_primary_float_closes_that_document() {
    let mut failures = Vec::new();
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        let id = viewport(&h, 1);
        focus(&mut h, &b, id, ppp);
        h.state_mut().save_as(Some("saved-document-b.psd".into())).unwrap();
        assert!(!h.state().session.active().unwrap().is_dirty());
        let first = h.state().session.documents()[0].doc.clone();
        b.borrow_mut().close.insert(id);
        h.step();
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("primary-clean-close-{ppp}x"));
        if h.state().session.documents().len() != 1 || h.state().session.documents()[0].doc != first {
            failures.push(format!("{ppp}x primary-close retained document; docs={}", h.state().session.documents().len()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn closing_a_dirty_primary_float_uses_the_unsaved_prompt() {
    let mut failures = Vec::new();
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        h.state_mut().run("layer.new.layer", serde_json::json!({})).unwrap();
        let id = viewport(&h, 1);
        focus(&mut h, &b, id, ppp);
        assert!(h.state().session.active().unwrap().is_dirty());
        let docs: Vec<_> = h.state().session.documents().iter().map(|d| d.doc.clone()).collect();
        b.borrow_mut().close.insert(id);
        h.step();
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("primary-dirty-close-{ppp}x"));
        assert_eq!(h.state().session.documents().iter().map(|d| d.doc.clone()).collect::<Vec<_>>(), docs, "close request itself never discards model data");
        if h.state().discard.is_none() || h.state().ui.windows.len() != 2 {
            failures.push(format!(
                "{ppp}x primary-close omitted prompt/removed view: prompt={},windows={}",
                h.state().discard.is_some(),
                h.state().ui.windows.len()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

fn pencil(h: &mut Harness) {
    h.state_mut().ui.tool = state::Tool::Pencil;
    h.state_mut().run("tools.setColors", serde_json::json!({"foreground":[1.0,0.0,0.0,1.0]})).unwrap();
    h.state_mut().run("tools.setBrush", serde_json::json!({"brush":{"size":6.0,"pressureSize":false,"smoothing":{"amount":0.0}}})).unwrap();
    h.run_steps(2);
}
fn stroke(from: egui::Pos2, to: egui::Pos2) -> [Vec<egui::Event>; 3] {
    [
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
        ],
        vec![egui::Event::PointerMoved(to)],
        vec![egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
    ]
}
fn command_key(key: egui::Key) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers { ctrl: true, command: true, ..Default::default() } }
}
#[test]
fn input_control_root_pointer_and_keyboard_run_once() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, true, ppp);
        assert_eq!(control_done(&mut h, "ui.set", serde_json::json!({"panels":{"properties":false}}))["ok"], true);
        pencil(&mut h);
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let xf = canvas::ViewXform::active(h.state()).unwrap();
        for events in stroke(xf.to_screen(78.0, 78.0), xf.to_screen(103.0, 78.0)) {
            h.input_mut().events.extend(events);
            h.step();
        }
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("root-input-painted-{ppp}x"));
        let docs = h.state().session.documents();
        assert_eq!(docs[0].doc, before[0].0, "root input does not modify other document");
        assert_eq!(docs[0].history.past_len(), before[0].1);
        assert_eq!(docs[1].doc.layers[0].surface().unwrap().rgba(90, 78), [1.0, 0.0, 0.0, 1.0], "real root pointer paints same analytical native pixel");
        assert_eq!(docs[1].history.past_len(), before[1].1 + 1, "one gesture, one commit");
        h.input_mut().events.push(command_key(egui::Key::Z));
        h.step();
        h.run_steps(2);
        assert_eq!(h.state().session.documents()[1].doc, before[1].0, "real root Undo restores exact original document");
        assert_eq!(h.state().session.documents()[1].history.past_len(), before[1].1);
        h.input_mut().events.push(command_key(egui::Key::J));
        h.step();
        h.run_steps(2);
        capture(&mut h, &b, &r, &format!("root-input-key-{ppp}x"));
        assert_eq!(h.state().session.documents()[1].doc.layers.len(), before[1].0.layers.len() + 1, "root shortcut duplicates once");
        assert_eq!(h.state().session.documents()[1].history.past_len(), before[1].1 + 1);
        assert_eq!(h.state().session.documents()[0].doc, before[0].0);
    }
}
#[test]
fn input_control_primary_child_keyboard_targets_its_document_once() {
    let mut failures = Vec::new();
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        let id = viewport(&h, 0);
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        focus(&mut h, &b, id, ppp);
        b.borrow_mut().events.insert(id, vec![command_key(egui::Key::J)]);
        h.step();
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("child-input-key-{ppp}x"));
        let docs = h.state().session.documents();
        assert_eq!(docs[1].doc, before[1].0, "child command never leaks into other document");
        assert_eq!(docs[1].history.past_len(), before[1].1);
        eprintln!(
            "4092 child-key theme={theme:?} ppp={ppp} active={:?} layers={:?} past={:?}",
            h.state().session.active_index(),
            docs.iter().map(|d| d.doc.layers.len()).collect::<Vec<_>>(),
            docs.iter().map(|d| d.history.past_len()).collect::<Vec<_>>()
        );
        if h.state().session.active_index() != Some(0)
            || docs[0].doc.layers.len() != before[0].0.layers.len() + 1
            || docs[0].history.past_len() != before[0].1 + 1
        {
            failures.push(format!("{theme:?}/{ppp}x primary child key neither targeted nor executed once"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}
#[test]
fn input_control_additional_child_focus_never_retargets_tools() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, true, ppp);
        h.state_mut().session.set_active(0);
        invoke(&mut h, "window.newWindowForDocument", serde_json::json!({}));
        h.state_mut().session.set_active(1);
        pencil(&mut h);
        let id = viewport(&h, 0);
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        focus(&mut h, &b, id, ppp);
        for events in stroke(egui::pos2(350.0, 250.0), egui::pos2(375.0, 250.0)) {
            b.borrow_mut().events.insert(id, events);
            h.step();
        }
        b.borrow_mut().events.insert(id, vec![command_key(egui::Key::J)]);
        h.step();
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("extra-input-focus-{ppp}x"));
        assert_eq!(h.state().session.active_index(), Some(1), "view-only extra focus does not change tool target");
        assert_eq!(
            h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(),
            before,
            "view-only child events leave all model/history unchanged"
        );
        assert_eq!(shown_tabs(&h), [0, 1]);
    }
}

#[test]
fn explicit_repeated_float_keeps_two_views_with_one_primary_and_atomic_errors() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, true, ppp);
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        invoke(&mut h, "window.arrange.floatInWindow", serde_json::json!({"document":0,"position":[1950.0,40.5]}));
        h.state_mut().ui.views[0].center = [128.0, 128.0];
        h.state_mut().ui.views[0].zoom = 2.0 * ppp;
        h.state_mut().ui.views[0].rotation = 23.0;
        h.state_mut().ui.views[0].fit_pending = false;
        h.run_steps(2);
        let old = h.state().ui.windows[0].clone();
        invoke(&mut h, "window.arrange.floatInWindow", serde_json::json!({}));
        capture(&mut h, &b, &r, &format!("repeated-float-{ppp}x"));
        let windows = &h.state().ui.windows;
        assert_eq!(windows.len(), 2, "both accepted explicit calls still create a view");
        assert_eq!(windows.iter().filter(|w| w.primary).count(), 1);
        assert!(!windows[0].primary && windows[1].primary);
        assert_eq!(windows[0].id, old.id);
        assert_eq!(windows[0].view, old.view, "demoted additional view retains its exact independent camera");
        assert_eq!(windows[0].position, old.position);
        assert_eq!(windows[1].position, None, "default accepted position remains OS-placed");
        let stable = windows.clone();
        let ctx = h.ctx.clone();
        for bad in [
            serde_json::json!({"document":2}),
            serde_json::json!({"document":-1}),
            serde_json::json!({"position":[1.0]}),
            serde_json::json!({"position":[1.0e30,0.0]}),
        ] {
            assert!(menus::invoke(h.state_mut(), &ctx, "window.arrange.floatInWindow", bad).is_err());
            assert_eq!(h.state().ui.windows, stable, "rejected params change neither role nor camera/position");
        }
        assert_eq!(
            h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(),
            before,
            "presentation commands preserve all document/history data"
        );
        assert_eq!(shown_tabs(&h), [1], "only the remaining docked primary has a tab");
    }
}

fn control_done(h: &mut Harness, method: &str, params: serde_json::Value) -> serde_json::Value {
    let ctx = h.ctx.clone();
    let (req, _rx) = control::ControlRequest::new(method, params);
    match control::handle(h.state_mut(), &ctx, &req) {
        control::Outcome::Done(value) => value,
        _ => panic!("{method} did not return an immediate outcome"),
    }
}
#[test]
fn control_input_pointer_uses_native_coordinates_for_floated_primary() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        let id = viewport(&h, 0);
        focus(&mut h, &b, id, ppp);
        pencil(&mut h);
        assert_eq!(h.state().session.active_index(), Some(0));
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let response = control_done(
            &mut h,
            "ui.pointer",
            serde_json::json!({"events":[{"kind":"down","x":78.0,"y":78.0},{"kind":"move","x":103.0,"y":78.0},{"kind":"up","x":103.0,"y":78.0}]}),
        );
        assert_eq!(response["ok"], true, "{response}");
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("control-pointer-{ppp}x"));
        let docs = h.state().session.documents();
        assert_eq!(docs[0].doc.layers[0].surface().unwrap().rgba(90, 78), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(docs[0].history.past_len(), before[0].1 + 1);
        assert_eq!((docs[1].doc.clone(), docs[1].history.past_len()), before[1]);
    }
}
#[test]
fn control_input_key_reaches_native_primary_exactly_once() {
    let mut failures = Vec::new();
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        let id = viewport(&h, 0);
        focus(&mut h, &b, id, ppp);
        h.run_steps(2);
        assert_eq!(h.state().session.active_index(), Some(0));
        assert!(!h.input().focused, "actual root remains unfocused; no focus workaround");
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = calls.clone();
        h.state_mut().services.automation_command = Some(Box::new(move |id, params| {
            observed.lock().unwrap().push((id.to_string(), params.clone()));
            Ok(())
        }));
        let ctx = h.ctx.clone();
        let (req, _rx) = control::ControlRequest::new("ui.key", serde_json::json!({"key":"J","ctrl":true}));
        assert!(matches!(control::handle(h.state_mut(), &ctx, &req), control::Outcome::AfterInput));
        // This is the documented host feed used when a harness does not call raw_input_hook.
        let queued = h.state_mut().take_synthetic_step();
        assert_eq!(queued.len(), 2, "one press and one release");
        h.input_mut().events.extend(queued);
        h.step();
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("control-key-{ppp}x"));
        let docs = h.state().session.documents();
        assert_eq!((docs[1].doc.clone(), docs[1].history.past_len()), before[1], "agent key never leaks into background doc");
        eprintln!(
            "4092 control-key {theme:?}/{ppp}x layers={:?} past={:?} policycalls={:?}",
            docs.iter().map(|d| d.doc.layers.len()).collect::<Vec<_>>(),
            docs.iter().map(|d| d.history.past_len()).collect::<Vec<_>>(),
            calls.lock().unwrap()
        );
        // Stock menus::invoke and app.run_command both authorize; the ordinary tabbed
        // control proves this exact sequence. Dispatch is counted by model/history edits.
        if docs[0].doc.layers.len() != before[0].0.layers.len() + 1
            || docs[0].history.past_len() != before[0].1 + 1
            || *calls.lock().unwrap() != vec![("layer.new.layerViaCopy".to_string(), serde_json::json!({})); 2]
        {
            failures.push(format!("{theme:?}/{ppp}x agent key lost or dispatched more than once"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

fn plain_key(key: egui::Key) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
}
fn feed_key(h: &mut Harness, key: egui::Key) {
    h.input_mut().events.push(plain_key(key));
    h.step();
    h.run_steps(2);
}
#[test]
fn control_close_additional_and_unknown_are_atomic_and_keep_document() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, true, ppp);
        h.state_mut().run("layer.new.layer", serde_json::json!({})).unwrap();
        invoke(&mut h, "window.newWindowForDocument", serde_json::json!({}));
        let windows = h.state().ui.windows.clone();
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = calls.clone();
        h.state_mut().services.automation_command = Some(Box::new(move |id, _| {
            observed.lock().unwrap().push(id.to_string());
            Err("denied".into())
        }));
        let response = control_done(&mut h, "ui.window.close", serde_json::json!({"window":u64::MAX}));
        assert_eq!(response["ok"], false);
        assert_eq!(h.state().ui.windows, windows);
        assert!(h.state().discard.is_none());
        let response = control_done(&mut h, "ui.window.close", serde_json::json!({"window":windows[0].id}));
        assert_eq!(response["ok"], true, "additional close is pure view state: {response}");
        assert!(h.state().ui.windows.is_empty() && h.state().discard.is_none());
        assert!(calls.lock().unwrap().is_empty(), "view-only close never invokes file.close policy");
        assert_eq!(h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(), before);
        h.run_steps(2);
        capture(&mut h, &b, &r, &format!("control-close-extra-{ppp}x"));
    }
}
#[test]
fn control_close_clean_primary_uses_explicit_identity_and_policy_once() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        let id = h.state().ui.windows[0].id;
        let kept = h.state().session.documents()[1].doc.clone();
        assert_eq!(h.state().session.active_index(), Some(1), "another document is initially active");
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = calls.clone();
        h.state_mut().services.automation_command = Some(Box::new(move |id, params| {
            observed.lock().unwrap().push((id.to_string(), params.clone()));
            Ok(())
        }));
        let response = control_done(&mut h, "ui.window.close", serde_json::json!({"window":id}));
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(h.state().session.documents().len(), 1);
        assert_eq!(h.state().session.documents()[0].doc, kept);
        assert_eq!(h.state().session.documents()[0].history.past_len(), 0);
        assert_eq!(*calls.lock().unwrap(), [("file.close".to_string(), serde_json::json!({"document":0}))]);
        assert_eq!(h.state().ui.windows.len(), 1);
        assert!(h.state().ui.windows[0].primary && h.state().ui.windows[0].document == 0);
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("control-close-clean-{ppp}x"));
    }
}
#[test]
fn control_close_primary_denial_preserves_clean_and_dirty_views() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        h.state_mut().run("layer.new.layer", serde_json::json!({})).unwrap();
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let windows = h.state().ui.windows.clone();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = calls.clone();
        h.state_mut().services.automation_command = Some(Box::new(move |id, params| {
            observed.lock().unwrap().push((id.to_string(), params.clone()));
            Err("close denied".into())
        }));
        for (document, window) in windows.iter().enumerate() {
            let response = control_done(&mut h, "ui.window.close", serde_json::json!({"window":window.id}));
            assert_eq!(response["ok"], false, "{response}");
            assert_eq!(response["error"], "close denied");
            assert!(h.state().discard.is_none());
            assert_eq!(h.state().ui.windows, windows);
            assert_eq!(calls.lock().unwrap()[document], ("file.close".to_string(), serde_json::json!({"document":document})));
        }
        assert_eq!(h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(), before);
        h.run_steps(2);
        capture(&mut h, &b, &r, &format!("control-close-denied-{ppp}x"));
    }
}
#[test]
fn control_close_dirty_primary_cancel_and_accept_stay_pinned_after_reorder() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        h.state_mut().run("layer.new.layer", serde_json::json!({})).unwrap();
        let closing = h.state().session.documents()[1].doc.id;
        let window = h.state().ui.windows[1].id;
        let active = viewport(&h, 0);
        focus(&mut h, &b, active, ppp);
        pencil(&mut h);
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let windows = h.state().ui.windows.clone();
        let response = control_done(&mut h, "ui.window.close", serde_json::json!({"window":window}));
        assert_eq!(response["ok"], true, "{response}");
        assert!(h.state().discard.is_some());
        h.run_steps(2);
        // A native child is a different input layer: the root modal must block its tools too.
        for events in stroke(egui::pos2(350.0, 250.0), egui::pos2(375.0, 250.0)) {
            b.borrow_mut().events.insert(active, events);
            h.step();
        }
        b.borrow_mut().events.insert(active, vec![command_key(egui::Key::J)]);
        h.step();
        assert_eq!(
            h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(),
            before,
            "no child painting/commands underneath dirty modal"
        );
        assert_eq!(h.state().ui.windows, windows);
        focus(&mut h, &b, egui::ViewportId::ROOT, ppp);
        h.run_steps(2);
        capture(&mut h, &b, &r, &format!("control-close-dirty-prompt-{ppp}x"));
        feed_key(&mut h, egui::Key::Escape);
        assert!(h.state().discard.is_none());
        assert_eq!(h.state().ui.windows, windows, "Cancel retains all primary views/cameras");
        assert_eq!(h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(), before);
        assert_eq!(control_done(&mut h, "ui.window.close", serde_json::json!({"window":window}))["ok"], true);
        assert!(h.state().discard.is_some());
        h.state_mut().run("file.close", serde_json::json!({"document":0})).unwrap();
        h.state_mut().run("file.new", serde_json::json!({"name":"Document C","width":256,"height":256,"background":"white"})).unwrap();
        let kept = h.state().session.documents()[1].doc.clone();
        assert_eq!(h.state().session.active_index(), Some(1));
        assert_eq!(h.state().session.documents()[0].doc.id, closing, "pending target moved from index1 to index0");
        focus(&mut h, &b, egui::ViewportId::ROOT, ppp);
        h.run_steps(2);
        feed_key(&mut h, egui::Key::N); // Linux's actual No/Don't Save modal accelerator.
        assert!(h.state().discard.is_none());
        assert_eq!(h.state().session.documents().len(), 1);
        assert_eq!(h.state().session.documents()[0].doc, kept, "confirmation closes original DocId, not new active doc");
        assert_eq!(h.state().session.documents()[0].history.past_len(), 0);
        assert!(!h.state().ui.windows.iter().any(|w| w.id == window));
        capture(&mut h, &b, &r, &format!("control-close-accepted-reorder-{ppp}x"));
    }
}
#[test]
fn native_dirty_primary_emits_cancel_close_until_real_prompt_cancel() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        h.state_mut().run("layer.new.layer", serde_json::json!({})).unwrap();
        let id = viewport(&h, 1);
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let window = h.state().ui.windows[1].id;
        b.borrow_mut().close.insert(id);
        h.step();
        let canceled = b
            .borrow()
            .output
            .values()
            .chain(std::iter::once(h.output()))
            .any(|out| out.viewport_output.get(&id).is_some_and(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::CancelClose))));
        assert!(canceled, "actual child output cancels native Close while prompt is pending");
        assert!(h.state().discard.is_some());
        assert!(h.state().ui.windows.iter().any(|w| w.id == window && w.primary));
        eprintln!("4092 native-close {theme:?}/{ppp}x child={id:?} CancelClose={canceled}");
        focus(&mut h, &b, egui::ViewportId::ROOT, ppp);
        h.run_steps(2);
        capture(&mut h, &b, &r, &format!("native-close-pending-{ppp}x"));
        feed_key(&mut h, egui::Key::Escape);
        assert!(h.state().discard.is_none());
        assert!(h.state().ui.windows.iter().any(|w| w.id == window && w.primary));
        assert_eq!(h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(), before);
    }
}
#[test]
fn consolidate_and_close_others_preserve_canonical_primary_camera_and_history() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        h.state_mut().session.edit_prefs(|p| p.tools.overscroll = true);
        h.state_mut().ui.views[0].zoom = 1.5 * ppp;
        h.state_mut().ui.views[0].center = [120.0, 132.0];
        h.state_mut().ui.views[0].rotation = 23.0;
        h.run_steps(3);
        let kept = h.state().session.documents()[0].doc.clone();
        let camera = h.state().ui.views[0].clone();
        let window = h.state().ui.windows[0].id;
        focus(&mut h, &b, egui::ViewportId::ROOT, ppp);
        invoke(&mut h, "file.closeOthers", serde_json::json!({"document":0}));
        assert_eq!(h.state().session.documents().len(), 1);
        assert_eq!(h.state().session.documents()[0].doc, kept);
        assert_eq!(h.state().session.documents()[0].history.past_len(), 0);
        assert_eq!(h.state().ui.views[0], camera);
        assert_eq!(h.state().ui.windows.len(), 1);
        assert_eq!(h.state().ui.windows[0].id, window);
        assert!(h.state().ui.windows[0].primary);
        invoke(&mut h, "window.arrange.consolidateAllToTabs", serde_json::json!({}));
        assert!(h.state().ui.windows.is_empty());
        assert_eq!(shown_tabs(&h), [0]);
        assert_eq!(h.state().ui.views[0], camera, "native camera survives presentation transfer");
        assert_eq!(h.state().session.documents()[0].doc, kept);
        assert_eq!(h.state().session.documents()[0].history.past_len(), 0);
        capture(&mut h, &b, &r, &format!("consolidate-primary-{ppp}x"));
    }
}
#[test]
fn control_synthetic_key_denial_reaches_policy_once_without_child_edit() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        let id = viewport(&h, 0);
        focus(&mut h, &b, id, ppp);
        h.run_steps(2);
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = calls.clone();
        h.state_mut().services.automation_command = Some(Box::new(move |id, params| {
            observed.lock().unwrap().push((id.to_string(), params.clone()));
            Err("layer command denied".into())
        }));
        let ctx = h.ctx.clone();
        let (req, _rx) = control::ControlRequest::new("ui.key", serde_json::json!({"key":"J","ctrl":true}));
        assert!(matches!(control::handle(h.state_mut(), &ctx, &req), control::Outcome::AfterInput));
        let events = h.state_mut().take_synthetic_step();
        h.input_mut().events.extend(events);
        h.step();
        h.run_steps(2);
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(calls.lock().unwrap()[0].0, "layer.new.layerViaCopy");
        assert!(h.state().ui.status_error);
        assert_eq!(h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(), before);
        assert!(!h.input().focused, "authorization is honored without any root-focus workaround");
        capture(&mut h, &b, &r, &format!("synthetic-key-denied-{ppp}x"));
    }
}

#[test]
fn native_primary_gesture_keeps_owner_until_release_across_child_focus() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        let a = viewport(&h, 0);
        let other = viewport(&h, 1);
        focus(&mut h, &b, a, ppp);
        pencil(&mut h);
        let untouched = h.state().session.documents()[1].doc.clone();
        let initial = h.state().session.documents()[0].history.past_len();
        let [down, moved, up] = stroke(egui::pos2(350.0, 250.0), egui::pos2(375.0, 250.0));
        b.borrow_mut().events.insert(a, down);
        h.step();
        b.borrow_mut().events.insert(a, moved);
        h.step();
        focus(&mut h, &b, other, ppp);
        h.step();
        assert_eq!(h.state().session.active_index(), Some(0), "focus changes cannot retarget a held native gesture");
        assert_eq!(h.state().session.documents()[0].history.past_len(), initial, "live stroke has not committed yet");
        assert_eq!(h.state().session.documents()[1].doc, untouched);
        capture(&mut h, &b, &r, &format!("gesture-held-other-focus-{ppp}x"));
        // Native pointer capture delivers the release to the viewport which started the drag.
        b.borrow_mut().events.insert(a, up);
        h.step();
        h.run_steps(2);
        assert_eq!(h.state().session.documents()[0].doc.layers[0].surface().unwrap().rgba(90, 78), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(h.state().session.documents()[0].history.past_len(), initial + 1);
        assert_eq!(h.state().session.documents()[1].doc, untouched);
        assert_eq!(h.state().session.documents()[1].history.past_len(), 0);
        assert_eq!(h.state().session.active_index(), Some(1), "new focused primary may own input after release");
        capture(&mut h, &b, &r, &format!("gesture-released-other-focus-{ppp}x"));
    }
}
#[test]
fn mixed_docked_tab_keeps_real_index_and_floating_geometry_until_canvas_activation() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, false, ppp);
        invoke(&mut h, "window.arrange.consolidateAllToTabs", serde_json::json!({}));
        focus(&mut h, &b, egui::ViewportId::ROOT, ppp);
        invoke(&mut h, "window.arrange.floatInWindow", serde_json::json!({"document":0}));
        let a = viewport(&h, 0);
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        assert_eq!(shown_tabs(&h), [1]);
        assert_eq!(h.state().session.active_index(), Some(0));
        assert_eq!(h.state().last_canvas_rect.size(), egui::vec2(800.0, 600.0), "background docked presentation does not publish floating owner's geometry");
        let tab = h.state().tab_strip.as_ref().unwrap().tabs[0].1.center();
        for events in [
            vec![
                egui::Event::PointerMoved(tab),
                egui::Event::PointerButton { pos: tab, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            ],
            vec![egui::Event::PointerButton { pos: tab, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
        ] {
            h.input_mut().events.extend(events);
            h.step();
        }
        h.run_steps(2);
        assert_eq!(h.state().session.active_index(), Some(1), "compact tab slot0 activates real document1");
        assert_ne!(h.state().last_canvas_rect.size(), egui::vec2(800.0, 600.0), "root primary publishes its own geometry when active");
        focus(&mut h, &b, a, ppp);
        h.run_steps(2);
        assert_eq!(h.state().session.active_index(), Some(0));
        assert_eq!(h.state().last_canvas_rect.size(), egui::vec2(800.0, 600.0));
        assert_eq!(shown_tabs(&h), [1]);
        assert_eq!(h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect::<Vec<_>>(), before);
        capture(&mut h, &b, &r, &format!("mixed-primary-return-{ppp}x"));
    }
}

#[test]
fn ordinary_tabbed_synthetic_key_authorization_control() {
    let mut failures = Vec::new();
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        documents(&mut h, &b, true, ppp);
        h.state_mut().session.set_active(0);
        focus(&mut h, &b, egui::ViewportId::ROOT, ppp);
        h.run_steps(2);
        assert_eq!(h.state().session.active_index(), Some(0));
        assert!(h.input().focused, "ordinary focused root control");
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = calls.clone();
        h.state_mut().services.automation_command = Some(Box::new(move |id, params| {
            observed.lock().unwrap().push((id.to_string(), params.clone()));
            Ok(())
        }));
        let ctx = h.ctx.clone();
        let (req, _rx) = control::ControlRequest::new("ui.key", serde_json::json!({"key":"J","ctrl":true}));
        assert!(matches!(control::handle(h.state_mut(), &ctx, &req), control::Outcome::AfterInput));
        // This is the documented host feed used when a harness does not call raw_input_hook.
        let queued = h.state_mut().take_synthetic_step();
        assert_eq!(queued.len(), 2, "one press and one release");
        h.input_mut().events.extend(queued);
        h.step();
        h.run_steps(3);
        capture(&mut h, &b, &r, &format!("tabbed-control-key-{ppp}x"));
        let docs = h.state().session.documents();
        assert_eq!((docs[1].doc.clone(), docs[1].history.past_len()), before[1], "agent key never leaks into background doc");
        eprintln!(
            "4092 tabbed-control-key {theme:?}/{ppp}x layers={:?} past={:?} policycalls={:?}",
            docs.iter().map(|d| d.doc.layers.len()).collect::<Vec<_>>(),
            docs.iter().map(|d| d.history.past_len()).collect::<Vec<_>>(),
            calls.lock().unwrap()
        );
        if docs[0].doc.layers.len() != before[0].0.layers.len() + 1
            || docs[0].history.past_len() != before[0].1 + 1
            || *calls.lock().unwrap() != vec![("layer.new.layerViaCopy".to_string(), serde_json::json!({})); 2]
        {
            failures.push(format!("{theme:?}/{ppp}x ordinary root key model/history or inherited authorization sequence differed"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn embedded_primary_pointer_native_key_and_synthetic_key_edit_once() {
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        h.ctx.set_embed_viewports(true);
        documents(&mut h, &b, true, ppp);
        invoke(&mut h, "window.arrange.floatInWindow", serde_json::json!({"document":0}));
        h.state_mut().ui.views[0].center = [128.0, 128.0];
        h.state_mut().ui.views[0].zoom = ppp;
        h.state_mut().ui.views[0].doc_size = [256, 256];
        h.state_mut().ui.views[0].fit_pending = false;
        pencil(&mut h);
        h.run_steps(3);
        assert!(h.ctx.embed_viewports() && h.input().focused);
        assert_eq!(b.borrow().passes, 0, "actual embedded fallback never invokes native backend");
        assert_eq!(shown_tabs(&h), [1]);
        assert_eq!(h.state().session.active_index(), Some(0));
        let before: Vec<_> = h.state().session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len())).collect();
        let prefs = h.state().session.prefs().clone();
        let xf = canvas::ViewXform::active(h.state()).unwrap();
        let (from, to) = (xf.to_screen(78.0, 78.0), xf.to_screen(103.0, 78.0));
        assert!(xf.rect.contains(from) && xf.rect.contains(to), "native stroke is in actual embedded canvas");
        assert_eq!(h.ctx.layer_id_at(from).unwrap().order, egui::Order::Middle, "pointer hits actual embedded Window layer");
        for events in stroke(from, to) {
            h.input_mut().events.extend(events);
            h.step();
        }
        h.run_steps(2);
        assert_eq!(h.state().session.active_index(), Some(0));
        assert_eq!(h.state().session.documents()[0].doc.layers[0].surface().unwrap().rgba(90, 78), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(h.state().session.documents()[0].history.past_len(), before[0].1 + 1);
        capture(&mut h, &b, &r, &format!("embedded-pointer-{ppp}x"));
        // Genuine key lives in the shared root context: only the embedded primary handles it.
        h.input_mut().events.push(command_key(egui::Key::J));
        h.step();
        h.run_steps(2);
        assert_eq!(h.state().session.documents()[0].doc.layers.len(), before[0].0.layers.len() + 1);
        assert_eq!(h.state().session.documents()[0].history.past_len(), before[0].1 + 2);
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = calls.clone();
        h.state_mut().services.automation_command = Some(Box::new(move |id, params| {
            observed.lock().unwrap().push((id.to_string(), params.clone()));
            Ok(())
        }));
        let ctx = h.ctx.clone();
        let (req, _rx) = control::ControlRequest::new("ui.key", serde_json::json!({"key":"J","ctrl":true}));
        assert!(matches!(control::handle(h.state_mut(), &ctx, &req), control::Outcome::AfterInput));
        let queued = h.state_mut().take_synthetic_step();
        assert_eq!(queued.len(), 2);
        h.input_mut().events.extend(queued);
        h.step();
        h.run_steps(2);
        assert_eq!(h.state().session.documents()[0].doc.layers.len(), before[0].0.layers.len() + 2);
        assert_eq!(h.state().session.documents()[0].history.past_len(), before[0].1 + 3);
        assert_eq!(*calls.lock().unwrap(), vec![("layer.new.layerViaCopy".to_string(), serde_json::json!({})); 2]);
        assert_eq!((h.state().session.documents()[1].doc.clone(), h.state().session.documents()[1].history.past_len()), before[1]);
        assert_eq!(*h.state().session.prefs(), prefs);
        assert_eq!(b.borrow().passes, 0);
        capture(&mut h, &b, &r, &format!("embedded-native-and-synthetic-key-{ppp}x"));
    }
}

// Monitor detection follows native window identity, independently of the editing owner.
// The backend runs the real immediate viewport callback; only the platform reader is supplied.
#[derive(Clone, Copy, Debug)]
enum MonitorPresentation {
    Primary,
    Additional,
    Root,
    Embedded,
}
#[derive(Debug, PartialEq)]
struct MonitorUntouched {
    documents: Vec<(std::sync::Arc<photocraft_doc::Document>, usize, bool)>,
    preferences: photocraft_engine::prefs::Preferences,
    cameras: Vec<state::View>,
    windows: Vec<state::DocWindow>,
    active: Option<usize>,
    tool: state::Tool,
    ppp: f32,
    canvas_rect: egui::Rect,
    live_stroke: bool,
    automation_input: bool,
}
fn monitor_untouched(h: &Harness) -> MonitorUntouched {
    let app = h.state();
    MonitorUntouched {
        documents: app.session.documents().iter().map(|d| (d.doc.clone(), d.history.past_len(), d.is_dirty())).collect(),
        preferences: app.session.prefs().clone(),
        cameras: app.ui.views.clone(),
        windows: app.ui.windows.clone(),
        active: app.session.active_index(),
        tool: app.ui.tool,
        ppp: app.ppp,
        canvas_rect: app.last_canvas_rect,
        live_stroke: app.live_stroke.is_some(),
        automation_input: app.automation_input,
    }
}
fn monitor_displays(profile: &str, new_display: bool) -> Vec<photocraft_engine::display_color::Display> {
    use photocraft_engine::display_color::Display;
    let icc = photocraft_engine::color_cmds::resolve_profile(profile, None, None).unwrap().to_bytes();
    let mut displays = vec![
        Display { id: 1, name: "Root display".into(), frame: [0.0, 0.0, 1728.0, 1117.0], profile_name: Some(profile.into()), icc: Some(icc.clone()) },
        Display { id: 2, name: "Document display".into(), frame: [1728.0, -667.0, 3008.0, 1692.0], profile_name: Some(profile.into()), icc: Some(icc.clone()) },
    ];
    if new_display {
        displays.push(Display {
            id: 3,
            name: "New document display".into(),
            frame: [8000.0, 0.0, 1728.0, 1117.0],
            profile_name: Some(profile.into()),
            icc: Some(icc),
        });
    }
    displays
}
fn monitor_platform_geometry(h: &mut Harness, b: &BackendRef, moved: Option<egui::ViewportId>) {
    let mut b = b.borrow_mut();
    for (id, info) in &mut b.viewports {
        let (at, monitor) = if *id == egui::ViewportId::ROOT {
            (egui::pos2(100.0, 100.0), egui::vec2(1728.0, 1117.0))
        } else if Some(*id) == moved {
            (egui::pos2(9000.0, 100.0), egui::vec2(1728.0, 1117.0))
        } else {
            (egui::pos2(2000.0, -500.0), egui::vec2(3008.0, 1692.0))
        };
        info.outer_rect = Some(egui::Rect::from_min_size(at, egui::vec2(800.0, 600.0)));
        info.monitor_size = Some(monitor);
    }
    h.input_mut().viewports = b.viewports.iter().map(|(id, info)| (*id, info.clone())).collect();
}
fn monitor_capture(h: &mut Harness, b: &BackendRef, r: &Renderer, name: &str, ppp: f32) {
    assert!(b.borrow().errors.is_empty(), "{:?}", b.borrow().errors);
    let name = format!("{}-monitor-{name}-{ppp}x", h.state().ui.theme.id());
    let root = h.render().expect("actual root monitor frame");
    assert_eq!(root.dimensions(), ((800.0 * ppp) as u32, (600.0 * ppp) as u32));
    let directory = std::env::var_os("PHOTOCRAFT_FLOATING_DOCUMENT_EVIDENCE_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = &directory {
        std::fs::create_dir_all(dir).unwrap();
        root.save(dir.join(format!("{name}-root.png"))).unwrap();
    }
    for (id, out) in &b.borrow().output {
        let image = r.borrow_mut().as_mut().unwrap().render(&h.ctx, out).expect("actual immediate child monitor frame");
        assert_eq!(image.dimensions(), root.dimensions(), "unchanged native viewport geometry");
        assert_eq!(image.get_pixel(image.width() / 2, image.height() / 2).0, [255, 255, 255, 255], "white native document renders through its monitor profile");
        if let Some(dir) = &directory {
            image.save(dir.join(format!("{name}-child-{id:?}.png"))).unwrap();
        }
    }
}
fn monitor_detection_path(presentation: MonitorPresentation, changed_display: bool) {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    let mut failures = Vec::new();
    for (theme, ppp) in cases() {
        let (mut h, b, r, _integration) = harness(ppp, theme);
        if matches!(presentation, MonitorPresentation::Embedded) {
            h.ctx.set_embed_viewports(true);
        }
        documents(&mut h, &b, matches!(presentation, MonitorPresentation::Root), ppp);
        let target = match presentation {
            MonitorPresentation::Primary => viewport(&h, 0),
            MonitorPresentation::Additional => {
                let primary = viewport(&h, 0);
                focus(&mut h, &b, primary, ppp);
                h.run_steps(3);
                invoke(&mut h, "window.newWindowForDocument", serde_json::json!({}));
                let extra = h.state().ui.windows.iter().find(|w| !w.primary).unwrap();
                egui::ViewportId::from_hash_of(("docwin", extra.id))
            }
            MonitorPresentation::Root | MonitorPresentation::Embedded => egui::ViewportId::ROOT,
        };
        // Establish the same editing owner before leaving the app, so focus detection itself
        // cannot be mistaken for document selection or a shared-geometry change.
        let editing_owner = if matches!(presentation, MonitorPresentation::Additional) { viewport(&h, 0) } else { target };
        focus(&mut h, &b, editing_owner, ppp);
        h.run_steps(3);
        monitor_platform_geometry(&mut h, &b, None);
        let initial_displays = monitor_displays("srgb", false);
        monitor_status::apply(h.state_mut(), Ok(initial_displays.clone()));
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = reads.clone();
        let (give, receivers) = mpsc::channel::<mpsc::Sender<monitor_status::Detection>>();
        let give = Mutex::new(give);
        h.state_mut().services.read_displays = Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = mpsc::channel();
            give.lock().unwrap().send(tx).unwrap();
            Some(rx)
        }));
        let outside = egui::ViewportId::from_hash_of("another application has focus");
        focus(&mut h, &b, outside, ppp);
        h.input_mut().time = Some(100.0);
        h.run_steps(4);
        assert_eq!(reads.load(Ordering::SeqCst), 0, "stable displays and no focus never poll periodically");
        assert!(!h.ctx.input(|i| i.focused));
        assert!(b.borrow().viewports.values().all(|v| v.focused == Some(false)));
        let before = monitor_untouched(&h);
        monitor_capture(&mut h, &b, &r, &format!("{presentation:?}-display-change-{changed_display}-before"), ppp);
        if changed_display {
            monitor_platform_geometry(&mut h, &b, Some(target));
        } else {
            focus(&mut h, &b, target, ppp);
        }
        // The first real child callback asks; the subsequent root pass starts the reader.
        h.step();
        h.step();
        let started = reads.load(Ordering::SeqCst);
        let response = receivers.try_recv().ok();
        monitor_capture(&mut h, &b, &r, &format!("{presentation:?}-display-change-{changed_display}-pending"), ppp);
        assert_eq!(monitor_untouched(&h), before, "reader trigger preserves model/history/preferences/cameras/input state");
        for (doc, prior) in h.state().session.documents().iter().zip(&before.documents) {
            assert!(Arc::ptr_eq(&doc.doc, &prior.0), "monitor detection never replaces document storage");
        }
        assert_eq!(h.state().session.color.displays, initial_displays, "delayed platform receiver has not answered");
        h.run_steps(4);
        assert_eq!(reads.load(Ordering::SeqCst), started, "at most one reading runs while its answer is pending");
        assert_eq!(h.state().session.color.displays, initial_displays);
        if let Some(response) = response {
            let delivered = monitor_displays("display-p3", changed_display);
            response.send(Ok(delivered.clone())).unwrap();
            h.run_steps(3);
            assert_eq!(h.state().session.color.displays, delivered, "whole-app root poll applies the actual delayed result");
            assert_eq!(h.state().session.color.monitor_for(Some(if changed_display { 3 } else { 2 })).description, "Display P3");
            assert_eq!(reads.load(Ordering::SeqCst), 1);
        } else {
            failures.push(format!("{theme:?}/{ppp}x {presentation:?} changed_display={changed_display}: native callback omitted the platform read"));
        }
        h.run_steps(3);
        monitor_capture(&mut h, &b, &r, &format!("{presentation:?}-display-change-{changed_display}-after"), ppp);
        assert_eq!(monitor_untouched(&h), before, "applied monitor profile preserves all editing ownership/model state");
        assert!(h.state().live_stroke.is_none());
        assert!(b.borrow().events.is_empty());
        assert_eq!(h.state().session.color.main_display, Some(1), "root display identity remains independent of the child");
        if matches!(presentation, MonitorPresentation::Primary | MonitorPresentation::Additional) {
            assert!(!h.ctx.input(|i| i.focused), "native child focus never changes root focus");
            assert!(b.borrow().passes > 0 && b.borrow().output.contains_key(&target), "actual immediate native child ran and rendered");
        } else if matches!(presentation, MonitorPresentation::Embedded) {
            assert_eq!(b.borrow().passes, 0, "actual embedded fallback does not invent native child passes");
        }
        eprintln!(
            "4092-monitor {theme:?}/{ppp}x presentation={presentation:?} changed_display={changed_display} root_focused={} reads={started} delayed_result={} child_passes={}",
            h.ctx.input(|i| i.focused),
            started == 1,
            b.borrow().passes
        );
        if started != 1 {
            failures.push(format!("{theme:?}/{ppp}x {presentation:?} expected one platform read, got {started}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn monitor_primary_native_focus_return_reads_displays_with_unfocused_root() {
    monitor_detection_path(MonitorPresentation::Primary, false);
}
#[test]
fn monitor_primary_native_unknown_display_reads_displays_without_root_input() {
    monitor_detection_path(MonitorPresentation::Primary, true);
}
#[test]
fn monitor_additional_native_focus_keeps_existing_detection_and_camera() {
    monitor_detection_path(MonitorPresentation::Additional, false);
}
#[test]
fn monitor_root_focus_keeps_stock_detection_without_native_children() {
    monitor_detection_path(MonitorPresentation::Root, false);
}
#[test]
fn monitor_embedded_focus_uses_root_detection_and_preserves_editing_owner() {
    monitor_detection_path(MonitorPresentation::Embedded, false);
}
