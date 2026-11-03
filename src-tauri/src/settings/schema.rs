//! Persistent settings schema. Mirrored in TypeScript by
//! `src/schemas/settings.ts` (zod). Keep field names + `serde(rename_all)`
//! choices in sync between the two.
//!
//! Defaults here are the source of truth: the frontend never invents a
//! value the backend hasn't seen.

use serde::{Deserialize, Serialize};

/// Which transcription engine to dispatch dictation through.
///
/// `local` — bundled / downloaded GGML model via whisper-rs.
/// `scribe` — ElevenLabs Scribe v2 over WebSocket (Phase 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    #[default]
    Local,
    Scribe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalSettings {
    /// Active model id (`tiny | base | small | medium | large-v3-turbo`).
    pub model: String,
    /// Subset of catalog ids whose .bin files are present on disk.
    pub installed_models: Vec<String>,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            model: "small".into(),
            // No bundled model in Phase 5+. `installed_models` starts empty;
            // the user installs via the Models tab UI downloader.
            installed_models: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScribeSettings {
    /// Pointer; the actual key lives in macOS Keychain (Phase 7).
    pub has_api_key: bool,
    /// v1 only ships streaming. Field exists so settings.json doesn't need
    /// migration when batch becomes a future option.
    pub streaming: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    #[default]
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeySettings {
    pub mode: HotkeyMode,
    /// Display string. Phase 8 turns this into a structured chord;
    /// for now the engine ignores it and hardcodes F5.
    pub binding: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            mode: HotkeyMode::Hold,
            binding: "F5".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    pub local_max_seconds: u32,
    pub scribe_max_seconds: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            local_max_seconds: 60,
            scribe_max_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PillSettings {
    pub follow_cursor: bool,
}

impl Default for PillSettings {
    fn default() -> Self {
        Self {
            follow_cursor: true,
        }
    }
}

/// Optional audible feedback on dictation start/stop.
///
/// Uses macOS system sounds (NSSound) so no audio assets are shipped.
/// Both default to `false` — users opt in from the About tab.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoundsSettings {
    /// Play a system sound when dictation begins (start of hold/toggle).
    pub start_enabled: bool,
    /// Play a system sound when dictation ends cleanly (text injected).
    pub stop_enabled: bool,
}

/// Audio input device configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AudioSettings {
    /// Optional input device name (as returned by `list_input_devices`).
    /// If `None`, the system default device is used.
    /// If the device name is not found, the system default is used as a fallback.
    pub input_device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub engine: EngineKind,
    pub local: LocalSettings,
    pub scribe: ScribeSettings,
    pub hotkey: HotkeySettings,
    /// `auto` for whisper-driven detection, otherwise BCP-47 tag.
    pub language: String,
    pub limits: Limits,
    pub pill: PillSettings,
    pub sounds: SoundsSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    pub telemetry: bool,
    pub first_run_completed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine: EngineKind::default(),
            local: LocalSettings::default(),
            scribe: ScribeSettings::default(),
            hotkey: HotkeySettings::default(),
            language: "auto".into(),
            limits: Limits::default(),
            pill: PillSettings::default(),
            sounds: SoundsSettings::default(),
            audio: AudioSettings::default(),
            telemetry: false,
            first_run_completed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_json() {
        let s = Settings::default();
        let v = serde_json::to_value(&s).expect("serialize");
        let back: Settings = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back.language, "auto");
        assert!(matches!(back.engine, EngineKind::Local));
        assert!(matches!(back.hotkey.mode, HotkeyMode::Hold));
        assert_eq!(back.hotkey.binding, "F5");
        assert_eq!(back.limits.local_max_seconds, 60);
        assert_eq!(back.limits.scribe_max_seconds, 300);
    }

    #[test]
    fn engine_kind_serializes_snake_case() {
        let v = serde_json::to_value(EngineKind::Scribe).expect("serialize scribe");
        assert_eq!(v, serde_json::json!("scribe"));
    }
}
