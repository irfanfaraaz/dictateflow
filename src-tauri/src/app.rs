//! AppState holder. Wired into Tauri at startup; commands access it via
//! tauri::State<'_, AppState>.

#[derive(Default)]
pub struct AppState {
    // Modules will register fields here as we add features.
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
}
