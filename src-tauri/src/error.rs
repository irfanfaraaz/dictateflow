//! App-wide error type for Tauri command handlers.
//!
//! IPC commands return Result<T, AppError>. AppError serializes to a string
//! that the frontend can display. Internal modules use their own thiserror
//! enums and convert via `?`.

use serde::Serialize;
use thiserror::Error;

#[allow(dead_code)] // wired up in later phases
#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

#[allow(dead_code)] // wired up in later phases
pub type AppResult<T> = std::result::Result<T, AppError>;
