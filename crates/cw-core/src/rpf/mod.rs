//! The RPF7 archive layer: the virtual filesystem everything else reads through.
//!
//! Port of `CodeWalker.Core/GameFiles/RpfFile.cs` and `RpfManager.cs`.
//!
//! An RPF is a flat table of 16-byte entries plus a name blob, both optionally
//! encrypted, describing a directory tree whose payloads are raw-deflate blobs
//! at 512-byte sector offsets. Archives nest: a `.rpf` stored as a binary entry
//! is itself an archive at an offset inside its parent's physical file.
//!
//! [`RpfManager`] flattens a whole game folder into one addressable index, so a
//! caller sees a single tree rather than 179 separate archives.

pub mod entry;
pub mod file;
pub mod manager;
pub mod mapped;

pub use entry::{
    size_from_flags, BinaryEntry, DirectoryEntry, EntryKind, ResourceEntry, RpfEncryption, RpfEntry,
};
pub use file::{RpfFile, RPF7, SECTOR};
pub use manager::{stamp_file, EntryRef, RpfManager, ScanError};
pub use mapped::{ArchiveMaps, MAX_LIVE_MAPPINGS};
