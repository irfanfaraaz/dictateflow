//! Static catalog of supported GGML models.
//!
//! Phase 2 only ships `small`; Phase 5 fills in the rest and wires the
//! download UI. The catalog lives here so other modules can list it without
//! importing model-management plumbing.

#![allow(dead_code)]

/// A single catalog entry. Phase 5 will surface this in the Models tab.
#[derive(Debug, Clone, Copy)]
pub struct ModelEntry {
    /// Short id used in URLs / filenames (`ggml-<id>.bin`).
    pub id: &'static str,
    /// Human-readable name for the UI.
    pub display_name: &'static str,
    /// Approximate on-disk size in MiB. Used for download UI hints.
    pub size_mib: u32,
    /// Direct HuggingFace download URL.
    pub url: &'static str,
    /// SHA-256 hex digest of the file. Verified post-download in Phase 5.
    pub sha256: &'static str,
}

/// The model bundled / supported in Phase 2.
///
/// Phase 5 adds: tiny, base, medium, large-v3.
pub const SMALL: ModelEntry = ModelEntry {
    id: "small",
    display_name: "Whisper Small (multilingual)",
    size_mib: 466,
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
    sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
};

/// All catalog entries. For Phase 2 this is just `[SMALL]`; the array exists
/// so Phase 5 can iterate without changing the public surface.
pub const ALL: &[ModelEntry] = &[SMALL];

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
}
