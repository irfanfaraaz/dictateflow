/**
 * Static catalog of GGML models available for download.
 *
 * Mirrors `src-tauri/src/models/catalog.rs`. Keep ids + size_mb in sync;
 * URLs and sha256 are Rust-only (download happens server-side).
 */

export interface ModelEntry {
  id: string;
  displayName: string;
  /** Approximate size in megabytes. */
  sizeMb: number;
  description: string;
  /** Short phrase for "recommended for" copy. */
  recommendedFor: string;
}

export const MODELS: ModelEntry[] = [
  {
    id: "tiny",
    displayName: "Whisper Tiny",
    sizeMb: 75,
    description: "multilingual",
    recommendedFor: "quick notes, battery-critical use",
  },
  {
    id: "base",
    displayName: "Whisper Base",
    sizeMb: 142,
    description: "multilingual",
    recommendedFor: "fast transcription with decent accuracy",
  },
  {
    id: "small",
    displayName: "Whisper Small",
    sizeMb: 466,
    description: "multilingual",
    recommendedFor: "best speed / accuracy balance",
  },
  {
    id: "medium",
    displayName: "Whisper Medium",
    sizeMb: 1533,
    description: "multilingual",
    recommendedFor: "higher accuracy, longer dictation",
  },
  {
    id: "large-v3-turbo",
    displayName: "Whisper Large v3 Turbo",
    sizeMb: 1624,
    description: "multilingual",
    recommendedFor: "highest accuracy with turbo speed",
  },
];

/** Look up a catalog entry by id. */
export function findModel(id: string): ModelEntry | undefined {
  return MODELS.find((m) => m.id === id);
}
