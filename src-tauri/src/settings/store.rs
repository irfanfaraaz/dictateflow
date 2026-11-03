//! `SettingsStore` — the single source of truth for persisted, non-secret
//! preferences. Wraps `tauri-plugin-store` for disk persistence and keeps a
//! hot in-memory `RwLock<Settings>` for cheap reads.
//!
//! # Thread-safety / atomicity
//!
//! Every mutation goes through [`SettingsStore::transact`], which holds a
//! single writer `Mutex` across read → modify → save. Two concurrent updates
//! (e.g. two model downloads finishing at once) therefore serialize instead
//! of each building on the same stale snapshot. The in-memory snapshot is
//! only replaced after the disk write succeeded, so memory and disk never
//! diverge on a failed save.
//!
//! No lock is ever held across an `.await` — every method here is sync.
//!
//! # Robust loading
//!
//! `tauri-plugin-store` silently ignores JSON syntax errors when it loads a
//! file, so we read the raw bytes ourselves first. The stored object is
//! merged over `Settings::default()` (so missing / newly-added fields take
//! their defaults and unknown fields are ignored). If the file still can't
//! be understood, the raw bytes are copied to
//! `settings.json.corrupt-<unix_ts>` before defaults are written, so a user's
//! settings are never silently destroyed.
//!
//! # Why we don't hold the `Store<R>` handle
//!
//! `Store<R: Runtime>` is generic over the Tauri runtime. Holding it on
//! `SettingsStore` would force the same generic onto every consumer.
//! Instead, the plugin caches stores by path internally — we re-fetch the
//! handle via `app.store("settings.json")` each call, which is a hashmap
//! lookup, not a disk read.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::StoreExt;
use thiserror::Error;
use tracing::{error, info, warn};

use super::schema::{Settings, CURRENT_SETTINGS_VERSION};

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
    #[error("settings file has no `{ROOT_KEY}` object")]
    MissingRoot,
}

/// Hot in-memory snapshot + a thin wrapper around the disk store.
pub struct SettingsStore {
    state: RwLock<Settings>,
    /// Serializes writers across read → merge → save.
    write_lock: Mutex<()>,
}

impl SettingsStore {
    /// Open (or create) the on-disk store and seed the in-memory snapshot.
    ///
    /// A missing file yields defaults. A file that can't be parsed is backed
    /// up to `settings.json.corrupt-<unix_ts>` and replaced by defaults.
    /// The (normalized) settings are always written back so newly-added
    /// fields and the current `version` are persisted.
    pub fn load<R: Runtime>(app: &AppHandle<R>) -> Result<Self, SettingsError> {
        let file_path = app.path().app_data_dir().ok().map(|d| d.join(STORE_FILE));

        // Read raw bytes BEFORE the plugin opens the file: the plugin swallows
        // parse errors and would hand us an empty store.
        let raw = file_path.as_deref().and_then(read_raw);

        // We save explicitly after every mutation; the plugin's debounced
        // auto-save would race our rollback logic.
        let store = app.store_builder(STORE_FILE).disable_auto_save().build()?;

        let settings = match raw {
            None => {
                info!("no settings.json yet — using defaults");
                Settings::default()
            }
            Some(bytes) => match parse_store_bytes(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        ?e,
                        "settings.json unreadable — backing up and using defaults"
                    );
                    if let Some(p) = file_path.as_deref() {
                        backup_corrupt(p, &bytes);
                    }
                    Settings::default()
                }
            },
        };

        store.set(ROOT_KEY, serde_json::to_value(&settings)?);
        store.save()?;

        Ok(Self {
            state: RwLock::new(settings),
            write_lock: Mutex::new(()),
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
        self.transact(app, |current| {
            let mut value = serde_json::to_value(current)?;
            merge(&mut value, patch);
            Ok(serde_json::from_value(value)?)
        })
    }

    /// Atomically mutate the settings with `f` and persist the result.
    ///
    /// `f` sees the latest snapshot (no other writer can interleave), so
    /// read-modify-write sequences such as "add id to `installed_models`"
    /// cannot lose concurrent updates.
    pub fn update_with<R: Runtime, F>(
        &self,
        app: &AppHandle<R>,
        f: F,
    ) -> Result<Settings, SettingsError>
    where
        F: FnOnce(&mut Settings),
    {
        self.transact(app, |current| {
            let mut next = current.clone();
            f(&mut next);
            Ok(next)
        })
    }

    /// Core write path: hold the writer lock across read → compute → save,
    /// and only publish the new snapshot to memory after a successful save.
    fn transact<R: Runtime, F>(&self, app: &AppHandle<R>, f: F) -> Result<Settings, SettingsError>
    where
        F: FnOnce(&Settings) -> Result<Settings, SettingsError>,
    {
        let _writer = match self.write_lock.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };

        let current = self.get();
        let next = f(&current)?;
        let next_value = serde_json::to_value(&next)?;

        let store = app.store(STORE_FILE)?;
        store.set(ROOT_KEY, next_value);
        if let Err(e) = store.save() {
            // Roll the plugin's cache back so a later save can't persist a
            // value we reported as failed.
            match serde_json::to_value(&current) {
                Ok(prev) => store.set(ROOT_KEY, prev),
                Err(se) => error!(?se, "settings rollback serialize failed"),
            }
            return Err(e.into());
        }

        {
            let mut guard = match self.state.write() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *guard = next.clone();
        }

        Ok(next)
    }
}

/// Read the raw store file. `None` when it doesn't exist (or is unreadable,
/// in which case there's nothing we could back up anyway).
fn read_raw(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warn!(?e, path = %path.display(), "settings.json read failed");
            None
        }
    }
}

/// Copy an unreadable settings file aside so the user's data isn't lost.
fn backup_corrupt(path: &Path, bytes: &[u8]) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = backup_path(path, ts);
    match std::fs::write(&backup, bytes) {
        Ok(()) => warn!(backup = %backup.display(), "backed up unreadable settings.json"),
        Err(e) => error!(?e, backup = %backup.display(), "failed to back up settings.json"),
    }
}

fn backup_path(path: &Path, ts: u64) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| STORE_FILE.into());
    name.push(format!(".corrupt-{ts}"));
    path.with_file_name(name)
}

/// Parse the full store file (`{"settings": {...}}`) into `Settings`.
fn parse_store_bytes(bytes: &[u8]) -> Result<Settings, SettingsError> {
    let root: Value = serde_json::from_slice(bytes)?;
    let stored = root.get(ROOT_KEY).ok_or(SettingsError::MissingRoot)?;
    if !stored.is_object() {
        return Err(SettingsError::MissingRoot);
    }
    settings_from_stored(stored).map_err(SettingsError::from)
}

/// Deserialize a stored settings object by merging it over the defaults, so
/// missing fields keep their defaults and unknown fields are ignored.
fn settings_from_stored(stored: &Value) -> Result<Settings, serde_json::Error> {
    let mut base = serde_json::to_value(Settings::default())?;
    merge(&mut base, stored.clone());
    let mut settings: Settings = serde_json::from_value(base)?;
    migrate(&mut settings);
    Ok(settings)
}

/// Schema migrations. Version 1 is the first versioned schema; files written
/// before versioning existed (no `version` key → default) are already
/// compatible with it.
fn migrate(settings: &mut Settings) {
    if settings.version > CURRENT_SETTINGS_VERSION {
        warn!(
            stored = settings.version,
            current = CURRENT_SETTINGS_VERSION,
            "settings.json written by a newer DictateFlow — keeping known fields"
        );
        return;
    }
    // Future: `if settings.version < 2 { ... }`
    settings.version = CURRENT_SETTINGS_VERSION;
}

/// Recursive JSON merge: object keys from `patch` overwrite (and recurse
/// into) the same keys in `base`; arrays and scalars replace wholesale.
/// A `null` never replaces an object (it would make the section
/// undeserializable); `null` over a scalar / optional is kept.
fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (Value::Object(_), Value::Null) => {}
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

    #[test]
    fn merge_null_does_not_clobber_object() {
        let mut base = json!({ "o": { "k": 1 }, "s": 2 });
        merge(&mut base, json!({ "o": null, "s": null }));
        assert_eq!(base, json!({ "o": { "k": 1 }, "s": null }));
    }

    #[test]
    fn old_file_missing_fields_keeps_other_values() {
        // Simulates a file written before `audio`, `sounds` and
        // `local.installed_models` existed.
        let stored = json!({
            "engine": "scribe",
            "local": { "model": "tiny" },
            "hotkey": { "mode": "toggle", "binding": "F6" },
            "language": "de",
            "telemetry": true,
            "first_run_completed": true
        });
        let s = settings_from_stored(&stored).expect("deserialize");
        assert!(matches!(s.engine, super::super::schema::EngineKind::Scribe));
        assert_eq!(s.local.model, "tiny");
        assert!(s.local.installed_models.is_empty());
        assert_eq!(s.hotkey.binding, "F6");
        assert_eq!(s.language, "de");
        assert!(s.telemetry);
        assert!(s.first_run_completed);
        assert_eq!(s.limits.local_max_seconds, 60);
        assert!(s.pill.follow_cursor);
        assert!(s.audio.input_device_id.is_none());
        assert_eq!(s.version, CURRENT_SETTINGS_VERSION);
    }

    #[test]
    fn unknown_extra_fields_are_ignored() {
        let stored = json!({
            "language": "fr",
            "some_future_field": { "x": 1 },
            "pill": { "follow_cursor": false, "future_pill_flag": true }
        });
        let s = settings_from_stored(&stored).expect("deserialize");
        assert_eq!(s.language, "fr");
        assert!(!s.pill.follow_cursor);
    }

    #[test]
    fn corrupt_json_is_an_error() {
        assert!(parse_store_bytes(b"{ \"settings\": { \"language\": ").is_err());
        assert!(parse_store_bytes(b"not json at all").is_err());
        assert!(parse_store_bytes(b"{}").is_err());
        assert!(parse_store_bytes(b"{\"settings\": 5}").is_err());
    }

    #[test]
    fn wrong_type_is_an_error() {
        let bytes = br#"{"settings": {"limits": {"local_max_seconds": "sixty"}}}"#;
        assert!(parse_store_bytes(bytes).is_err());
    }

    #[test]
    fn valid_file_round_trips() {
        let s = Settings {
            language: "es".into(),
            local: super::super::schema::LocalSettings {
                model: "base".into(),
                installed_models: vec!["base".into()],
            },
            ..Settings::default()
        };
        let file = json!({ "settings": serde_json::to_value(&s).expect("ser") });
        let bytes = serde_json::to_vec(&file).expect("bytes");
        let back = parse_store_bytes(&bytes).expect("parse");
        assert_eq!(back.language, "es");
        assert_eq!(back.local.installed_models, vec!["base".to_string()]);
    }

    #[test]
    fn backup_path_appends_suffix() {
        let p = backup_path(Path::new("/tmp/x/settings.json"), 42);
        assert_eq!(p, PathBuf::from("/tmp/x/settings.json.corrupt-42"));
    }
}
