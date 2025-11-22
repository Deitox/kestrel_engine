use std::sync::Arc;

use super::{App, ScriptConsoleEntry, ScriptConsoleKind, SCRIPT_CONSOLE_CAPACITY, SCRIPT_HISTORY_CAPACITY};

impl App {
    pub(super) fn push_script_console(&mut self, kind: ScriptConsoleKind, text: impl Into<String>) {
        let mut state = self.editor_ui_state_mut();
        state.script_console.push_back(ScriptConsoleEntry { kind, text: text.into() });
        while state.script_console.len() > SCRIPT_CONSOLE_CAPACITY {
            state.script_console.pop_front();
        }
        state.script_console_snapshot = None;
    }

    pub(super) fn script_console_entries(&mut self) -> Arc<[ScriptConsoleEntry]> {
        let mut state = self.editor_ui_state_mut();
        if let Some(cache) = &state.script_console_snapshot {
            return Arc::clone(cache);
        }
        let data = state.script_console.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        state.script_console_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub(super) fn script_repl_history_arc(&mut self) -> Arc<[String]> {
        let mut state = self.editor_ui_state_mut();
        if let Some(cache) = &state.script_repl_history_snapshot {
            return Arc::clone(cache);
        }
        let data = state.script_repl_history.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        state.script_repl_history_snapshot = Some(Arc::clone(&arc));
        arc
    }

    fn append_script_history(&mut self, command: &str) {
        if command.is_empty() {
            return;
        }
        let mut state = self.editor_ui_state_mut();
        state.script_repl_history.push_back(command.to_string());
        while state.script_repl_history.len() > SCRIPT_HISTORY_CAPACITY {
            state.script_repl_history.pop_front();
        }
        state.script_repl_history_index = None;
        state.script_repl_history_snapshot = None;
    }

    pub(super) fn execute_repl_command(&mut self, command: String) {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return;
        }
        self.append_script_history(trimmed);
        self.push_script_console(ScriptConsoleKind::Input, format!("> {trimmed}"));
        {
            let mut state = self.editor_ui_state_mut();
            state.script_repl_input.clear();
            state.script_focus_repl = true;
        }
        let result: Result<Option<String>, String> = if let Some(plugin) = self.script_plugin_mut() {
            match plugin.eval_repl(trimmed) {
                Ok(value) => Ok(value),
                Err(err) => {
                    let message = err.to_string();
                    plugin.set_error_message(message.clone());
                    Err(message)
                }
            }
        } else {
            Err("Script plugin unavailable; cannot evaluate command.".to_string())
        };
        match result {
            Ok(Some(value)) => self.push_script_console(ScriptConsoleKind::Output, value),
            Ok(None) => {}
            Err(message) => {
                self.push_script_console(ScriptConsoleKind::Error, message);
                let mut state = self.editor_ui_state_mut();
                state.script_debugger_open = true;
                state.script_focus_repl = true;
            }
        }
    }

    pub(super) fn sync_script_error_state(&mut self) {
        let current_error =
            self.script_plugin().and_then(|plugin| plugin.last_error().map(|err| err.to_string()));
        {
            let mut state = self.editor_ui_state_mut();
            if current_error == state.last_reported_script_error {
                return;
            }
            state.last_reported_script_error = current_error.clone();
        }
        if let Some(err) = current_error {
            self.push_script_console(ScriptConsoleKind::Error, format!("Runtime error: {err}"));
            let mut state = self.editor_ui_state_mut();
            state.script_debugger_open = true;
            state.script_focus_repl = true;
        }
    }
}
