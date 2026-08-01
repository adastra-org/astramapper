//! `cw-core` — a Rust port of `CodeWalker.Core`.
//!
//! Reference source: `adastra-org/CodeWalker` @ `d11e5951`, read in place from
//! `E:\Ad Astra Tools\CodeWalkerAstra`. See `docs/porting-guide.md`.
//!
//! The invariant across this crate: parsing is bit-exact. Given the same bytes,
//! every value, offset and name matches what the C# reader produces, and
//! parse-then-serialize with no edits reproduces the input byte for byte.
//! All game structs are little-endian, packed, and use `f32` — never widen a
//! serialized field to `f64`.

pub mod utils;

/// Errors raised while reading GTA V data.
#[derive(Debug, thiserror::Error)]
pub enum CwError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The bytes are not the format we were told to read.
    #[error("{what} is not valid: {detail}")]
    Invalid { what: &'static str, detail: String },
}

pub type Result<T> = std::result::Result<T, CwError>;
