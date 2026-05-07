//! Static catalog of supported GGML models.
//!
//! Phase 2 only shipped `small`; Phase 5 adds tiny, base, medium, and
//! large-v3-turbo with their HuggingFace URLs and SHA-256 digests.
//!
//! The catalog lives here so other modules can list it without importing
//! model-management plumbing.

/// A single catalog entry. Phase 5 surfaces this in the Models tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // display_name, size_mib, notes read by frontend via the TS catalog mirror
pub struct ModelEntry {
    /// Short id used in URLs / filenames (`ggml-<id>.bin`).
    pub id: &'static str,
    /// Human-readable name for the UI.
    pub display_name: &'static str,
    /// Approximate on-disk size in MiB. Used for download UI hints.
    pub size_mib: u32,
    /// Notes shown in the Models tab (speed/accuracy trade-off).
    pub notes: &'static str,
    /// Direct HuggingFace download URL.
    pub url: &'static str,
    /// SHA-256 hex digest of the file. Verified post-download in Phase 5.
    pub sha256: &'static str,
}

pub const TINY: ModelEntry = ModelEntry {
    id: "tiny",
    display_name: "Whisper Tiny (multilingual)",
    size_mib: 75,
    notes: "fastest, lowest accuracy — good for quick notes",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
    sha256: "bd577a113a864445d4c299885e0cb97d4ba92b5f0c2a4c3e25eda891e38aaef3",
};

pub const BASE: ModelEntry = ModelEntry {
    id: "base",
    display_name: "Whisper Base (multilingual)",
    size_mib: 142,
    notes: "fast with decent accuracy",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
    sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
};

pub const SMALL: ModelEntry = ModelEntry {
    id: "small",
    display_name: "Whisper Small (multilingual)",
    size_mib: 466,
    notes: "best speed/accuracy balance — recommended",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
    sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
};

pub const MEDIUM: ModelEntry = ModelEntry {
    id: "medium",
    display_name: "Whisper Medium (multilingual)",
    size_mib: 1533,
    notes: "higher accuracy, slower — good for dictation",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
    sha256: "0fd9727b6e1217c2f614f9b698455c4ffd82463b4155b68d1e3b25fe18c3f8c2",
};

pub const LARGE_V3_TURBO: ModelEntry = ModelEntry {
    id: "large-v3-turbo",
    display_name: "Whisper Large v3 Turbo",
    size_mib: 1624,
    notes: "highest accuracy with turbo speed — best quality",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
    sha256: "4af2b29d7ec73d781377bfd1758ca957a807e9ffd9d20b8f52c9b5baaae20f27",
};

/// All catalog entries in display order.
pub const ALL: &[ModelEntry] = &[TINY, BASE, SMALL, MEDIUM, LARGE_V3_TURBO];

/// Look up a catalog entry by its id. Returns `None` for unknown ids.
pub fn find(id: &str) -> Option<&'static ModelEntry> {
    ALL.iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_entry_well_formed() {
        assert_eq!(SMALL.id, "small");
        assert_eq!(SMALL.sha256.len(), 64, "sha256 must be 64 hex chars");
        assert!(SMALL.url.starts_with("https://"));
    }

    #[test]
    fn catalog_ids_unique() {
        let mut ids: Vec<&str> = ALL.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len(), "duplicate ids in catalog");
    }

    #[test]
    fn all_sha256_are_64_hex_chars() {
        for entry in ALL {
            assert_eq!(
                entry.sha256.len(),
                64,
                "sha256 for {} is {} chars (expected 64)",
                entry.id,
                entry.sha256.len()
            );
            assert!(
                entry.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "sha256 for {} contains non-hex chars",
                entry.id
            );
        }
    }

    #[test]
    fn find_returns_correct_entry() {
        let entry = find("small").expect("small should be in catalog");
        assert_eq!(entry.id, "small");
        assert!(find("nonexistent").is_none());
    }
}
