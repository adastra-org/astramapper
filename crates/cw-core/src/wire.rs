//! The binary envelope compiled assets cross the IPC boundary in.
//!
//! Not a port of anything. The reference draws in-process and has no boundary
//! to cross; this one has a WebView on the other side of it.
//!
//! Everything used to cross as JSON: vertex buffers base64'd into strings,
//! textures decoded out of their block format into RGBA, re-encoded as PNG,
//! then base64'd on top of that. Measured over 600 real textures, the decode
//! plus PNG plus base64 costs 12.92s and allocates 443.9 MB of intermediate
//! RGBA that is thrown away; handing over the blocks as stored costs 19.58ms
//! and is *smaller* on the wire even carrying every mip against the PNG path's
//! mip 0 alone. That is the whole reason this module exists.
//!
//! The layout, all little-endian:
//!
//! ```text
//! 0    magic 'AMB0'      4
//! 4    version           2      ENVELOPE_VERSION
//! 6    kind              2      Kind
//! 8    flags             4      reserved, 0
//! 12   section_count     4
//! 16   meta_offset       4      into the whole buffer
//! 20   meta_length       4      UTF-8 JSON, not padded
//! 24   table_offset      4      section_count * SECTION_ENTRY_BYTES
//! 28   blob_offset       4      first section's alignment floor
//! 32   reserved          32     zero
//! 64   ...               meta, then the table, then the sections
//! ```
//!
//! Each section table entry is `{ offset: u32, length: u32 }` into the whole
//! buffer.
//!
//! **Sections start on a 16-byte boundary.** That is not cosmetic: the point of
//! the format is that the frontend can say `new Float32Array(buffer, offset, n)`
//! and get a view rather than a copy, and a `Float32Array` view throws unless
//! its offset is a multiple of 4. Sixteen covers every typed array we will ever
//! put here, plus the `Float64Array` we will not.
//!
//! **The metadata stays JSON inside a binary envelope, deliberately.** It is
//! exactly the part that churns as shader parameters, tangents and UV1 land,
//! and a JSON section makes that additive on both sides. The bytes that matter
//! — millions of floats, megabytes of blocks — are already zero-copy, and no
//! amount of schema work makes them faster.

use crate::{CwError, Result};

/// Identifies the format on sight, and in a hex dump.
pub const MAGIC: [u8; 4] = *b"AMB0";

/// Bumped when the header layout changes. A reader that sees a version it does
/// not know refuses rather than guesses; a cache that holds an older one treats
/// it as a miss.
pub const ENVELOPE_VERSION: u16 = 1;

/// Fixed header length. Sections are 16-byte aligned and this is a multiple of
/// 16, so the header never forces padding by itself.
pub const HEADER_BYTES: usize = 64;

/// One `{ offset, length }` pair in the section table.
pub const SECTION_ENTRY_BYTES: usize = 8;

/// What every section offset is rounded up to. See the module note.
pub const SECTION_ALIGN: usize = 16;

/// What an envelope carries. Stored in the header so a reader can refuse a
/// payload meant for a different consumer instead of misreading its metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Kind {
    /// A compiled model: geometry and the textures its parts reference.
    Model = 1,
    /// Textures alone.
    Textures = 2,
    /// Collision geometry from a `.ybn`.
    Collision = 3,
    /// AI path nodes and links from a `.ynd`.
    Paths = 4,
    /// A `.ymap`'s flattened placements, column by column.
    Placements = 5,
    /// The world index's partition: one row per cell, column by column.
    Cells = 6,
    /// A batch of cells: their placements, plus the models those cells need.
    CellData = 7,
}

impl Kind {
    pub fn from_raw(value: u16) -> Option<Self> {
        Some(match value {
            1 => Kind::Model,
            2 => Kind::Textures,
            3 => Kind::Collision,
            4 => Kind::Paths,
            5 => Kind::Placements,
            6 => Kind::Cells,
            7 => Kind::CellData,
            _ => return None,
        })
    }
}

/// Builds one envelope: metadata plus an ordered list of byte sections.
///
/// Sections are referred to by index from inside the metadata, so push order is
/// the contract between the two halves. [`Envelope::push`] returns the index it
/// assigned rather than making the caller count.
#[derive(Debug, Default)]
pub struct Envelope {
    sections: Vec<Vec<u8>>,
}

impl Envelope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a section, returning the index the metadata should name it by.
    pub fn push(&mut self, bytes: impl Into<Vec<u8>>) -> usize {
        self.sections.push(bytes.into());
        self.sections.len() - 1
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Total section bytes so far, for budgeting a response before finishing it.
    pub fn payload_bytes(&self) -> usize {
        self.sections.iter().map(Vec::len).sum()
    }

    /// Serialize into one buffer: header, metadata, section table, sections.
    ///
    /// `meta` is written as-is and is expected to be UTF-8 JSON; nothing here
    /// parses it, so the caller owns the schema.
    pub fn finish(self, kind: Kind, meta: &str) -> Vec<u8> {
        let table_offset = HEADER_BYTES + meta.len();
        let table_bytes = self.sections.len() * SECTION_ENTRY_BYTES;
        let blob_offset = align_up(table_offset + table_bytes);

        let mut offsets = Vec::with_capacity(self.sections.len());
        let mut cursor = blob_offset;
        for section in &self.sections {
            cursor = align_up(cursor);
            offsets.push((cursor as u32, section.len() as u32));
            cursor += section.len();
        }

        let mut out = Vec::with_capacity(cursor);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&ENVELOPE_VERSION.to_le_bytes());
        out.extend_from_slice(&(kind as u16).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // flags
        out.extend_from_slice(&(self.sections.len() as u32).to_le_bytes());
        out.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        out.extend_from_slice(&(table_offset as u32).to_le_bytes());
        out.extend_from_slice(&(blob_offset as u32).to_le_bytes());
        out.resize(HEADER_BYTES, 0);

        out.extend_from_slice(meta.as_bytes());
        for (offset, length) in &offsets {
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&length.to_le_bytes());
        }

        for (section, (offset, _)) in self.sections.iter().zip(&offsets) {
            out.resize(*offset as usize, 0);
            out.extend_from_slice(section);
        }
        out
    }
}

/// A parsed envelope, borrowing the buffer it was read from.
///
/// Exists for the round-trip tests and for `cw-cache`, which stores finished
/// envelopes and needs to know a stored blob is still readable. The frontend
/// has its own decoder in `src/lib/wire.ts`; the two are pinned against each
/// other by a fixture rather than by shared code, there being no shared
/// language to put it in.
#[derive(Debug)]
pub struct EnvelopeView<'a> {
    pub kind: Kind,
    pub meta: &'a str,
    sections: Vec<&'a [u8]>,
}

impl<'a> EnvelopeView<'a> {
    pub fn read(buffer: &'a [u8]) -> Result<Self> {
        if buffer.len() < HEADER_BYTES {
            return Err(invalid(format!(
                "{} bytes is shorter than the {HEADER_BYTES}-byte header",
                buffer.len()
            )));
        }
        if buffer[..4] != MAGIC {
            return Err(invalid("the magic is not AMB0".to_string()));
        }
        let version = u16::from_le_bytes([buffer[4], buffer[5]]);
        if version != ENVELOPE_VERSION {
            return Err(invalid(format!(
                "version {version} is not {ENVELOPE_VERSION}"
            )));
        }
        let raw_kind = u16::from_le_bytes([buffer[6], buffer[7]]);
        let kind = Kind::from_raw(raw_kind)
            .ok_or_else(|| invalid(format!("kind {raw_kind} is not one we write")))?;

        let count = read_u32(buffer, 12) as usize;
        let meta_offset = read_u32(buffer, 16) as usize;
        let meta_length = read_u32(buffer, 20) as usize;
        let table_offset = read_u32(buffer, 24) as usize;

        let meta_bytes = buffer
            .get(meta_offset..meta_offset + meta_length)
            .ok_or_else(|| invalid("the metadata runs past the buffer".to_string()))?;
        let meta = std::str::from_utf8(meta_bytes)
            .map_err(|e| invalid(format!("the metadata is not UTF-8: {e}")))?;

        let mut sections = Vec::with_capacity(count);
        for index in 0..count {
            let at = table_offset + index * SECTION_ENTRY_BYTES;
            if at + SECTION_ENTRY_BYTES > buffer.len() {
                return Err(invalid(
                    "the section table runs past the buffer".to_string(),
                ));
            }
            let offset = read_u32(buffer, at) as usize;
            let length = read_u32(buffer, at + 4) as usize;
            let section = buffer
                .get(offset..offset + length)
                .ok_or_else(|| invalid(format!("section {index} runs past the buffer")))?;
            sections.push(section);
        }

        Ok(Self {
            kind,
            meta,
            sections,
        })
    }

    pub fn section(&self, index: usize) -> Option<&'a [u8]> {
        self.sections.get(index).copied()
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }
}

fn align_up(value: usize) -> usize {
    value.div_ceil(SECTION_ALIGN) * SECTION_ALIGN
}

fn read_u32(buffer: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buffer[at], buffer[at + 1], buffer[at + 2], buffer[at + 3]])
}

fn invalid(detail: String) -> CwError {
    CwError::Invalid {
        what: "envelope",
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_metadata_and_sections() {
        let mut envelope = Envelope::new();
        assert_eq!(envelope.push(vec![1u8, 2, 3]), 0);
        assert_eq!(envelope.push(vec![9u8; 40]), 1);
        let bytes = envelope.finish(Kind::Model, r#"{"path":"a.ydr"}"#);

        let view = EnvelopeView::read(&bytes).expect("read");
        assert_eq!(view.kind, Kind::Model);
        assert_eq!(view.meta, r#"{"path":"a.ydr"}"#);
        assert_eq!(view.section_count(), 2);
        assert_eq!(view.section(0), Some(&[1u8, 2, 3][..]));
        assert_eq!(view.section(1), Some(&[9u8; 40][..]));
        assert_eq!(view.section(2), None);
    }

    /// The reason the format has padding at all. A `Float32Array` view over a
    /// misaligned offset throws, and the frontend takes views rather than
    /// copies, so this is the property the whole design rests on.
    #[test]
    fn every_section_starts_on_a_sixteen_byte_boundary() {
        // Deliberately awkward: a metadata length that is not a multiple of 16,
        // an odd section count, and section lengths that are not either.
        let mut envelope = Envelope::new();
        for length in [1usize, 7, 33, 100, 5] {
            envelope.push(vec![0xabu8; length]);
        }
        let bytes = envelope.finish(Kind::Textures, "{\"a\":1234567}");

        let count = read_u32(&bytes, 12) as usize;
        let table = read_u32(&bytes, 24) as usize;
        assert_eq!(count, 5);
        for index in 0..count {
            let offset = read_u32(&bytes, table + index * SECTION_ENTRY_BYTES) as usize;
            assert_eq!(offset % SECTION_ALIGN, 0, "section {index} at {offset}");
        }
    }

    #[test]
    fn an_empty_envelope_is_still_readable() {
        let bytes = Envelope::new().finish(Kind::Paths, "{}");
        let view = EnvelopeView::read(&bytes).expect("read");
        assert_eq!(view.section_count(), 0);
        assert_eq!(view.meta, "{}");
    }

    #[test]
    fn a_zero_length_section_keeps_its_index() {
        let mut envelope = Envelope::new();
        envelope.push(vec![]);
        envelope.push(vec![7u8, 7]);
        let bytes = envelope.finish(Kind::Model, "{}");
        let view = EnvelopeView::read(&bytes).expect("read");
        assert_eq!(view.section(0), Some(&[][..]));
        assert_eq!(view.section(1), Some(&[7u8, 7][..]));
    }

    #[test]
    fn floats_survive_the_round_trip_exactly() {
        let values: Vec<f32> = vec![0.0, -0.0, 1.0, -2.5, f32::MIN_POSITIVE, 1e30, -1e-30];
        let mut raw = Vec::new();
        for value in &values {
            raw.extend_from_slice(&value.to_le_bytes());
        }
        let mut envelope = Envelope::new();
        envelope.push(raw);
        let bytes = envelope.finish(Kind::Model, "{}");

        let view = EnvelopeView::read(&bytes).expect("read");
        let section = view.section(0).expect("section");
        let back: Vec<f32> = section
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(back, values);
        assert!(back[1].is_sign_negative(), "-0.0 kept its sign");
    }

    #[test]
    fn refuses_a_truncated_buffer() {
        let bytes = Envelope::new().finish(Kind::Model, "{}");
        assert!(EnvelopeView::read(&bytes[..HEADER_BYTES - 1]).is_err());
    }

    #[test]
    fn refuses_the_wrong_magic() {
        let mut bytes = Envelope::new().finish(Kind::Model, "{}");
        bytes[0] = b'X';
        assert!(EnvelopeView::read(&bytes).is_err());
    }

    /// A version bump must read as a refusal, never as a misparse — the cache
    /// turns this into a miss.
    #[test]
    fn refuses_a_version_it_does_not_know() {
        let mut bytes = Envelope::new().finish(Kind::Model, "{}");
        bytes[4..6].copy_from_slice(&(ENVELOPE_VERSION + 1).to_le_bytes());
        assert!(EnvelopeView::read(&bytes).is_err());
    }

    #[test]
    fn refuses_a_section_pointing_past_the_buffer() {
        let mut envelope = Envelope::new();
        envelope.push(vec![1u8; 16]);
        let mut bytes = envelope.finish(Kind::Model, "{}");
        let table = read_u32(&bytes, 24) as usize;
        bytes[table + 4..table + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(EnvelopeView::read(&bytes).is_err());
    }
}
