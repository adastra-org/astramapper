//! `cw-cache` — a pack-file disk cache for compiled assets.
//!
//! **Status: the storage layer is here and tested; the `Cache` facade over it
//! is not written yet.** See `docs/phases.md` under phase 4 for where this sits
//! and what is meant to come next.
//!
//! # Why this crate exists at all
//!
//! Measured over a real install, opening a game folder re-scans 179 archives
//! and 403,642 entries every launch, and every streamed model re-extracts,
//! re-inflates and re-parses its source. Memory-mapping the archives bought
//! only 1.13x, because the run is dominated by deflate at about 380 MB/s. The
//! only thing that moves that number is not decompressing at all.
//!
//! # Why it is its own crate
//!
//! Not in `cw-core`, which is a port of `CodeWalker.Core` and a cache is not
//! part of that. Not in `src-tauri`, because the round trip has to be testable
//! without a Tauri runtime. `src-tauri` resolves the cache directory from
//! `app.path().app_cache_dir()` and passes it in, so this crate never learns
//! that an app exists — and it does not depend on `cw-core` either. It stores
//! opaque bytes under an opaque key; what the bytes mean is the caller's
//! business.
//!
//! # The three decisions
//!
//! - **Keyed by source, not by content** ([`key`]). Content-addressing means
//!   extracting to find out whether extraction was needed.
//! - **Pack files with a sidecar index** ([`pack`], [`index`]). One file per
//!   asset would be 400,000 files on NTFS for Defender to scan.
//! - **Whole-pack eviction.** Append-only packs cannot reclaim a hole without a
//!   compactor, and a compactor is a background rewrite of gigabytes that can
//!   corrupt the store, to recover space that costs nothing to leave alone.
//!
//! Each module's header carries the reasoning for its own half.

pub mod index;
pub mod key;
pub mod pack;

pub use key::{CacheKey, KeyInputs, Kind, FORMAT_VERSION};

/// What can go wrong reaching the cache.
///
/// Every variant is recoverable by treating the lookup as a miss and the write
/// as skipped. **Nothing in a cache failure may fail the operation that asked
/// for it**: this is derived data, and the caller can always do the work
/// itself. A cache that can break the app is worse than no cache.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not {what}: {source}")]
    Io {
        what: &'static str,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
