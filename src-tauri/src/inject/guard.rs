//! `ClipboardGuard` — a Drop-based scheduler that restores the saved
//! clipboard ~250 ms after the guard goes out of scope.
//!
//! Why Drop and not an explicit cleanup? Two reasons:
//!
//! 1. **Panic safety.** If the inject orchestrator panics between
//!    `set_text` and the keystroke post, we still want the user's prior
//!    clipboard restored. Drop runs on unwind; an `if let Err…` cleanup
//!    block would not.
//! 2. **No `defer` in Rust.** This is the idiomatic substitute.
//!
//! Why the +250 ms delay? The host application has to actually consume
//! our synthetic ⌘V before we yank the pasteboard out from under it.
//! Apps read the pasteboard *after* the keystroke event lands and is
//! processed; restoring synchronously races that consumption and pastes
//! the *prior* clipboard contents instead. 250 ms is comfortably above
//! the slowest mainstream app's paste path while still feeling
//! "instant" to the user.

use std::time::Duration;

use tracing::warn;

use super::clipboard::{self, ClipboardSnapshot};
use super::InjectError;

/// Restore-on-drop wrapper around a [`ClipboardSnapshot`].
///
/// Construct with [`ClipboardGuard::save`]. Drop schedules the restore
/// onto the current Tokio runtime — therefore the guard MUST be created
/// while a Tokio runtime is active (i.e. inside `#[tokio::main]` or a
/// `#[tokio::test]`). This is always the case when wired through Tauri's
/// async command handlers.
pub struct ClipboardGuard {
    /// `Option` so `Drop` can `take()` and move the snapshot into the
    /// spawned restore task.
    snapshot: Option<ClipboardSnapshot>,
    /// Cached at construction so Drop can spawn even from a sync context.
    runtime: tokio::runtime::Handle,
}

impl ClipboardGuard {
    /// Capture the current pasteboard. Caller MUST bind the result to a
    /// named variable (`let _guard = …`) — discarding with `let _ = …`
    /// drops the guard immediately and the restore runs before the paste.
    pub fn save() -> Result<Self, InjectError> {
        let snapshot = clipboard::save()?;
        Ok(Self {
            snapshot: Some(snapshot),
            runtime: tokio::runtime::Handle::current(),
        })
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        let Some(snapshot) = self.snapshot.take() else {
            return;
        };
        // Schedule the restore on the runtime. We cannot block-on here —
        // Drop may execute on the runtime thread itself. The returned
        // JoinHandle is intentionally dropped (fire-and-forget cleanup);
        // failures are logged inside the task.
        std::mem::drop(self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Err(e) = clipboard::restore(snapshot) {
                warn!(?e, "clipboard restore failed");
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms Drop fires on panic mid-flow. We can't easily assert the
    /// scheduled restore actually ran (it's on a tokio task, +250 ms),
    /// but `should_panic` proves the guard's destructor doesn't itself
    /// panic and that unwinding completes — which is the real safety
    /// property we care about.
    #[tokio::test]
    #[should_panic(expected = "intentional panic mid-injection")]
    async fn drop_runs_on_panic() {
        // We may not actually have a real clipboard available in CI; if
        // `save()` fails, fall back to constructing the guard from a
        // default snapshot directly so we still exercise the Drop path.
        let guard = ClipboardGuard::save().unwrap_or_else(|_| ClipboardGuard {
            snapshot: Some(ClipboardSnapshot::default()),
            runtime: tokio::runtime::Handle::current(),
        });
        let _bound = guard;
        panic!("intentional panic mid-injection");
    }
}
