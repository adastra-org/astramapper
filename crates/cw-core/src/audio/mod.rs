//! Audio: the AWC container, its codecs, and WAV output.
//!
//! Port of `CodeWalker.Core/GameFiles/FileTypes/AwcFile.cs`. `RelFile.cs` — the
//! `.rel` sound metadata that names and sequences these streams — is 1.2 MB and
//! still **todo**, so a stream is identified here by its hash, not its name.

pub mod adpcm;
pub mod awc;
pub mod wav;

pub use awc::{AwcFile, ChunkType, Codec, Format, Stream};
