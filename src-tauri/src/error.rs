//! App-wide error type for Tauri command handlers.
//!
//! IPC commands return Result<T, AppError>. AppError serializes to a string
//! that the frontend can display. Internal modules use their own thiserror
//! enums and convert via `?`.

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Other(#[from] anyhow::Error),
    #[error("settings: {0}")]
    Settings(#[from] crate::settings::SettingsError),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;
