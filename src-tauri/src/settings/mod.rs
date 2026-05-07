//! Persisted settings: schema definition + tauri-plugin-store wrapper.
//!
//! Frontend mirror lives at `src/schemas/settings.ts`. The shape on disk is
//! a single JSON object under the `settings` key; partial updates merge in
//! through `SettingsStore::update_patch`.

pub mod schema;
pub mod store;

pub use schema::Settings;
pub use store::{SettingsError, SettingsStore};
