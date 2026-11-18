//! Timestamped notes on standard error while HYPRBSD_NETWORK_TRACE is set.

use std::sync::OnceLock;
use std::time::Instant;

/// Writes `note`'s text, after the seconds since the first note, if HYPRBSD_NETWORK_TRACE is set.
pub fn trace(note: impl FnOnce() -> String) {
    static START: OnceLock<Option<Instant>> = OnceLock::new();
    if let Some(start) = START.get_or_init(|| std::env::var_os("HYPRBSD_NETWORK_TRACE").map(|_| Instant::now())) {
        eprintln!("hyprbsd-network: {:.3} {}", start.elapsed().as_secs_f64(), note());
    }
}
