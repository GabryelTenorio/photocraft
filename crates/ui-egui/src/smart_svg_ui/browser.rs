//! Browser builds retain internal Edit Contents without desktop editor sessions.

use serde_json::Value;

use crate::PhotocraftApp;

#[derive(Default)]
pub(crate) struct State {}

impl State {
    pub(crate) fn modal_open(&self) -> bool {
        false
    }
}

pub(crate) fn intercept(_app: &mut PhotocraftApp, _id: &str, _params: &Value) -> Option<Result<Value, String>> {
    None
}

pub(crate) fn poll(_app: &mut PhotocraftApp, _ctx: &egui::Context) {}

pub(crate) fn show(_app: &mut PhotocraftApp, _ctx: &egui::Context) {}
