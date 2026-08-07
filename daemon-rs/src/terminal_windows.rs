use std::{path::Path, sync::Arc};

use serde_json::{Value, json};

use crate::EventBus;

pub struct TerminalManager;

impl TerminalManager {
    pub fn new(_: Arc<EventBus>) -> Self {
        Self
    }

    pub fn list(&self, _: &str) -> Value {
        json!({"ok": true, "terminals": []})
    }

    pub fn open(&self, _: &Value, _: &Path) -> Result<Value, String> {
        Err("Terminal sessions require ConPTY and are not available on Windows yet.".into())
    }

    pub fn input(&self, _: &Value) -> Result<Value, String> {
        Err("Terminal sessions are not available on Windows.".into())
    }

    pub fn resize(&self, _: &Value) -> Result<Value, String> {
        Err("Terminal sessions are not available on Windows.".into())
    }

    pub fn kill(&self, _: &Value) -> Result<Value, String> {
        Ok(json!({"ok": true}))
    }
}
