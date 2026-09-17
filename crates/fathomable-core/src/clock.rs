//! The wall clock as Unix seconds, the unit every store timestamps in.
//!
//! Thread messages, review points, sessions, and board events all record when
//! they happened as whole seconds since the Unix epoch; this is the one place
//! that reads the clock for them.
//!
//! # Examples
//!
//! ```
//! let before = fathomable_core::clock::now();
//! let after = fathomable_core::clock::now();
//! assert!(after >= before);
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch; `0` when the clock is set before it.
#[must_use]
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
