//! Reading an RSC7 resource: the two-segment virtual address space its internal
//! pointers live in.
//!
//! Port of `ResourceDataReader` from
//! `CodeWalker.Core/GameFiles/Resources/ResourceData.cs`, plus the page-flag
//! helpers on `RpfResourceFileEntry` in `RpfFile.cs`.
//!
//! A resource payload is two concatenated halves — a *system* segment holding
//! structs and a *graphics* segment holding vertex, index and texture data. The
//! game maps them at two fixed bases and every pointer inside the file is an
//! address in that space, not an offset into the payload:
//!
//! ```text
//! 0x50000000 + n  ->  system[n]
//! 0x60000000 + n  ->  graphics[n]
//! ```
//!
//! So there is no relocation pass: resolving a pointer is masking off the base
//! and picking a segment. The discrimination is the reference's, a bitmask test
//! rather than a range check, tried system-first — reproduced exactly, because
//! the two masks overlap and the order decides who wins.

use crate::utils::DataReader;
use crate::{CwError, Result};

/// Base address of the system segment.
pub const SYSTEM_BASE: u64 = 0x5000_0000;
/// Base address of the graphics segment.
pub const GRAPHICS_BASE: u64 = 0x6000_0000;

/// Split a resource's page-flag word into a version nibble and a size.
///
/// Port of `RpfResourceFileEntry.GetVersionFromFlags`. The version is the top
/// nibble of each flag word: system high, graphics low. YDR is 165, YDD 165,
/// YFT 162, YTD 13.
pub fn version_from_flags(system_flags: u32, graphics_flags: u32) -> u32 {
    let sv = (system_flags >> 28) & 0xF;
    let gv = (graphics_flags >> 28) & 0xF;
    (sv << 4) + gv
}

/// A cursor over a resource payload, addressed the way the file's own pointers
/// address it.
pub struct ResourceReader<'a> {
    system: &'a [u8],
    graphics: &'a [u8],
    pos: u64,
}

impl<'a> ResourceReader<'a> {
    /// Split an extracted payload into its two segments.
    ///
    /// `system_size` comes from the entry's system page flags. The reference
    /// hands `MemoryStream` the same two ranges and lets a short payload throw;
    /// here a payload smaller than its flags claim is clamped instead, because
    /// the alternative is refusing to open a file CodeWalker opens fine.
    pub fn new(data: &'a [u8], system_size: usize) -> Self {
        let split = system_size.min(data.len());
        Self {
            system: &data[..split],
            graphics: &data[split..],
            pos: SYSTEM_BASE,
        }
    }

    pub fn system_len(&self) -> usize {
        self.system.len()
    }

    pub fn graphics_len(&self) -> usize {
        self.graphics.len()
    }

    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn set_position(&mut self, pos: u64) {
        self.pos = pos;
    }

    /// The segment and offset an address resolves to.
    ///
    /// Verbatim from `ReadFromStream`: `(pos & SYSTEM_BASE) == SYSTEM_BASE`
    /// first, then the graphics test. These are mask tests, so 0x70000000 would
    /// answer "system" — keeping the order keeps that behaviour rather than
    /// tightening it into something the file layout has never relied on.
    fn resolve(&self, pos: u64) -> Result<(&'a [u8], usize)> {
        if pos & SYSTEM_BASE == SYSTEM_BASE {
            return Ok((self.system, (pos & !SYSTEM_BASE) as usize));
        }
        if pos & GRAPHICS_BASE == GRAPHICS_BASE {
            return Ok((self.graphics, (pos & !GRAPHICS_BASE) as usize));
        }
        Err(CwError::Invalid {
            what: "resource pointer",
            detail: format!("{pos:#x} is in neither the system nor the graphics segment"),
        })
    }

    /// Read `count` bytes at the cursor.
    ///
    /// A read running past the end of a segment yields zeros for the tail and
    /// advances only as far as the segment goes, which is what `MemoryStream`
    /// does and what the reference therefore relies on. A truncated resource
    /// reads as zeros rather than failing the file.
    pub fn read_bytes(&mut self, count: usize) -> Result<Vec<u8>> {
        let (segment, offset) = self.resolve(self.pos)?;
        let mut out = vec![0u8; count];
        let available = segment.len().saturating_sub(offset.min(segment.len()));
        let taken = available.min(count);
        if taken > 0 {
            out[..taken].copy_from_slice(&segment[offset..offset + taken]);
        }
        self.pos += taken as u64;
        Ok(out)
    }

    /// Read a fixed-size struct field. Short reads are zeros, as above.
    fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.read_bytes(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_array::<1>()?[0])
    }

    pub fn read_u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.read_array()?))
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    pub fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.read_array()?))
    }

    pub fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    /// Three floats, as `ReadVector3` does. Note the reference's `Vector3` read
    /// consumes 12 bytes; the 4-byte pad after it in a struct is a separate
    /// named field, never part of this.
    pub fn read_vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.read_f32()?, self.read_f32()?, self.read_f32()?])
    }

    /// Run `f` with the cursor moved to `at`, then restore it.
    ///
    /// A null pointer yields `None` without calling `f` — the reference's
    /// `ReadBlockAt` returns `default(T)` for position 0, and every caller
    /// treats that as "this optional block is absent".
    pub fn at<T>(&mut self, at: u64, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if at == 0 {
            return Ok(None);
        }
        let saved = self.pos;
        self.pos = at;
        let result = f(self);
        self.pos = saved;
        result.map(Some)
    }

    pub fn read_bytes_at(&mut self, at: u64, count: usize) -> Result<Option<Vec<u8>>> {
        if count == 0 {
            return Ok(None);
        }
        self.at(at, |r| r.read_bytes(count))
    }

    pub fn read_u16s_at(&mut self, at: u64, count: usize) -> Result<Option<Vec<u16>>> {
        let Some(bytes) = self.read_bytes_at(at, count * 2)? else {
            return Ok(None);
        };
        Ok(Some(
            bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
        ))
    }

    pub fn read_u32s_at(&mut self, at: u64, count: usize) -> Result<Option<Vec<u32>>> {
        let Some(bytes) = self.read_bytes_at(at, count * 4)? else {
            return Ok(None);
        };
        Ok(Some(
            bytes
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        ))
    }

    pub fn read_u64s_at(&mut self, at: u64, count: usize) -> Result<Option<Vec<u64>>> {
        let Some(bytes) = self.read_bytes_at(at, count * 8)? else {
            return Ok(None);
        };
        Ok(Some(
            bytes
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect(),
        ))
    }

    pub fn read_f32s_at(&mut self, at: u64, count: usize) -> Result<Option<Vec<f32>>> {
        let Some(bytes) = self.read_bytes_at(at, count * 4)? else {
            return Ok(None);
        };
        Ok(Some(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        ))
    }

    /// `count` little-endian `Vector4`s (`[f32; 4]`, 16 bytes each) at `at`.
    pub fn read_vec4s_at(&mut self, at: u64, count: usize) -> Result<Option<Vec<[f32; 4]>>> {
        let Some(bytes) = self.read_bytes_at(at, count * 16)? else {
            return Ok(None);
        };
        Ok(Some(
            bytes
                .chunks_exact(16)
                .map(|c| {
                    let f = |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
                    [f(0), f(4), f(8), f(12)]
                })
                .collect(),
        ))
    }

    /// A NUL-terminated string at `at`, read out of whichever segment it lands
    /// in. Absent for a null pointer.
    pub fn read_string_at(&mut self, at: u64) -> Result<Option<String>> {
        if at == 0 {
            return Ok(None);
        }
        let (segment, offset) = self.resolve(at)?;
        if offset >= segment.len() {
            return Ok(None);
        }
        let mut reader = DataReader::new(&segment[offset..]);
        Ok(Some(reader.read_string()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From the reference: YDR files are version 165, and the nibbles are
    /// system-high / graphics-low.
    #[test]
    fn decodes_the_version_nibbles() {
        assert_eq!(version_from_flags(0xA000_0000, 0x5000_0000), 0xA5);
        assert_eq!(version_from_flags(0xA000_0000, 0x5000_0000), 165);
        assert_eq!(version_from_flags(0, 0), 0);
        assert_eq!(version_from_flags(0xF000_0000, 0xF000_0000), 0xFF);
    }

    fn reader(system: &[u8], graphics: &[u8]) -> Vec<u8> {
        let mut data = system.to_vec();
        data.extend_from_slice(graphics);
        data
    }

    #[test]
    fn addresses_the_two_segments_separately() {
        let data = reader(&[1, 2, 3, 4], &[9, 9, 9, 9]);
        let mut r = ResourceReader::new(&data, 4);

        assert_eq!(r.position(), SYSTEM_BASE);
        assert_eq!(r.read_u32().unwrap(), 0x0403_0201);

        r.set_position(GRAPHICS_BASE);
        assert_eq!(r.read_u32().unwrap(), 0x0909_0909);
    }

    #[test]
    fn resolves_pointers_by_base() {
        let data = reader(&[0xAA; 8], &[0xBB; 8]);
        let mut r = ResourceReader::new(&data, 8);

        r.set_position(SYSTEM_BASE + 4);
        assert_eq!(r.read_u8().unwrap(), 0xAA);
        r.set_position(GRAPHICS_BASE + 4);
        assert_eq!(r.read_u8().unwrap(), 0xBB);
    }

    #[test]
    fn rejects_an_address_in_neither_segment() {
        let data = reader(&[0; 4], &[0; 4]);
        let mut r = ResourceReader::new(&data, 4);
        r.set_position(0x1234);
        assert!(r.read_u32().is_err());
    }

    /// `MemoryStream.Read` short-reads rather than throwing, so the reference
    /// tolerates a resource whose flags claim more than the payload holds. The
    /// tail must come back as zeros.
    #[test]
    fn zero_fills_a_read_past_the_end_of_a_segment() {
        let data = reader(&[1, 2], &[]);
        let mut r = ResourceReader::new(&data, 2);
        assert_eq!(r.read_bytes(4).unwrap(), vec![1, 2, 0, 0]);
    }

    /// A payload shorter than its declared system size is clamped, not sliced
    /// out of range.
    #[test]
    fn clamps_a_system_size_larger_than_the_payload() {
        let data = vec![7u8; 4];
        let r = ResourceReader::new(&data, 64);
        assert_eq!(r.system_len(), 4);
        assert_eq!(r.graphics_len(), 0);
    }

    #[test]
    fn restores_the_cursor_after_a_seeked_read() {
        let data = reader(&[1, 2, 3, 4, 5, 6, 7, 8], &[]);
        let mut r = ResourceReader::new(&data, 8);
        r.set_position(SYSTEM_BASE + 4);

        let got = r.read_u32s_at(SYSTEM_BASE, 1).unwrap().unwrap();
        assert_eq!(got, vec![0x0403_0201]);
        assert_eq!(r.position(), SYSTEM_BASE + 4);
    }

    /// A null pointer is "absent", not address zero. `ReadBlockAt` returns
    /// `default(T)` for it and every caller reads that as an optional block.
    #[test]
    fn treats_a_null_pointer_as_absent() {
        let data = reader(&[1, 2, 3, 4], &[]);
        let mut r = ResourceReader::new(&data, 4);
        assert!(r.read_u32s_at(0, 1).unwrap().is_none());
        assert!(r.read_string_at(0).unwrap().is_none());
        assert!(r.at(0, |_| Ok(())).unwrap().is_none());
    }

    #[test]
    fn reads_a_string_out_of_the_segment_it_points_into() {
        let mut system = b"prop_bench_01a\0".to_vec();
        system.resize(32, 0);
        let data = reader(&system, &[]);
        let mut r = ResourceReader::new(&data, 32);
        assert_eq!(
            r.read_string_at(SYSTEM_BASE).unwrap().unwrap(),
            "prop_bench_01a"
        );
    }
}
