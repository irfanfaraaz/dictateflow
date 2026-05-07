//! `SettingsStore` — the single source of truth for persisted, non-secret
//! preferences. Wraps `tauri-plugin-store` for disk persistence and keeps a
//! hot in-memory `Arc<RwLock<Settings>>` for cheap reads.
//!
//! # Thread-safety
//!
//! Per CLAUDE.md, the `RwLockWriteGuard` is NEVER held across an `.await`.
//! `update()` mutates inside a sync block, drops the guard, then performs
//! its IO and returns the new clone. Callers emit IPC events after the
//! function returns.
//!
//! # Why we don't hold the `Store<R>` handle
//!
//! `Store<R: Runtime>` is generic over the Tauri runtime. Holding it on
//! `SettingsStore` would force the same generic onto every consumer.
//! Instead, the plugin caches stores by path internally — we re-fetch the
//! handle via `app.store("settings.json")` each call, which is a hashmap
//! lookup, not a disk read.

use std::sync::{Arc, RwLock};

use serde_json::Value;
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;
use thiserror::Error;
use tracing::warn;

use super::schema::Settings;

/// Filename under the app data dir; the plugin resolves the rest.
const STORE_FILE: &str = "settings.json";

/// Top-level key under which the whole `Settings` blob is stored. We keep
/// everything under one key so disk writes are atomic and the merge logic
/// stays at the Rust layer (the plugin's per-key API would split a single
/// logical update across multiple writes).
const ROOT_KEY: &str = "settings";

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("settings store io: {0}")]
    Store(#[from] tauri_plugin_store::Error),
    #[error("settings serde: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Hot in-memory snapshot + a thin wrapper around the disk store.
pub struct SettingsStore {
    state: Arc<RwLock<Settings>>,
}

impl SettingsStore {
    /// Open (or create) the on-disk store and seed the in-memory snapshot.
    ///
    /// A missing or corrupt file falls back to `Settings::default()` and
    /// writes the defaults back to disk so subsequent loads succeed.
    pub fn load<R: Runtime>(app: &AppHandle<R>) -> Result<Self, SettingsError> {
        let store = app.store(STORE_FILE)?;

        let settings = match store.get(ROOT_KEY) {
            Some(v) => match serde_json::from_value::<Settings>(v) {
                Ok(s) => s,
                Err(e) => {
                    warn!(?e, "settings.json corrupt — falling back to defaults");
                    let defaults = Settings::default();
                    store.set(ROOT_KEY, serde_json::to_value(&defaults)?);
                    store.save()?;
                    defaults
                }
            },
            None => {
                let defaults = Settings::default();
                store.set(ROOT_KEY, serde_json::to_value(&defaults)?);
                store.save()?;
                defaults
            }
        };

        Ok(Self {
            state: Arc::new(RwLock::new(settings)),
        })
    }

    /// Cheap clone of the current snapshot.
    pub fn get(&self) -> Settings {
        match self.state.read() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// Apply `patch` (a JSON object) on top of the current settings, persist
    /// the merged value, and return the new snapshot.
    ///
    /// `patch` may contain any subset of the schema; missing keys are left
    /// alone. Object values merge recursively; arrays and scalars replace.
    /// Invalid merges (e.g. a patch that produces a value that doesn't
    /// deserialize back into `Settings`) leave both disk and memory
    /// unchanged.
    pub fn update_patch<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        patch: Value,
    ) -> Result<Settings, SettingsError> {
        // 1. Build the proposed next value off the current snapshot. Done
        //    outside the write lock so a serde failure can't leave us
        //    half-applied.
        let current = self.get();
        let mut next_value = serde_json::to_value(&current)?;
        merge(&mut next_value, patch);
        let next: Settings = serde_json::from_value(next_value.clone())?;

        // 2. Swap in the new snapshot under the write lock. Tight scope,
        //    no `.await`, no IO inside.
        {
            let mut guard = match self.state.write() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *guard = next.clone();
        }

        // 3. Persist. We drop the in-memory lock before talking to the
        //    plugin so a slow flush can't block readers.
        let store = app.store(STORE_FILE)?;
        store.set(ROOT_KEY, next_value);
        store.save()?;

        Ok(next)
    }
}

/// Recursive JSON merge: object keys from `patch` overwrite (and recurse
/// into) the same keys in `base`; arrays and scalars replace wholesale.
fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (b, p) => *b = p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_overrides_scalar() {
        let mut base = json!({ "a": 1, "b": 2 });
        merge(&mut base, json!({ "a": 9 }));
        assert_eq!(base, json!({ "a": 9, "b": 2 }));
    }

    #[test]
    fn merge_recurses_into_objects() {
        let mut base = json!({ "outer": { "x": 1, "y": 2 } });
        merge(&mut base, json!({ "outer": { "y": 9, "z": 3 } }));
        assert_eq!(base, json!({ "outer": { "x": 1, "y": 9, "z": 3 } }));
    }

    #[test]
    fn merge_replaces_arrays_wholesale() {
        let mut base = json!({ "list": [1, 2, 3] });
        merge(&mut base, json!({ "list": [9] }));
        assert_eq!(base, json!({ "list": [9] }));
    }

    #[test]
    fn merge_creates_missing_keys() {
        let mut base = json!({});
        merge(&mut base, json!({ "new": { "k": "v" } }));
        assert_eq!(base, json!({ "new": { "k": "v" } }));
    }
}
