//! Little-endian reader over an in-memory buffer.
//!
//! Port of the `DataReader` half of `CodeWalker.Core/GameFiles/Utils/Data.cs`.
//!
//! The C# version wraps a `Stream` and supports big-endian for console files.
//! Everything AstraMapper targets is PC, which is little-endian throughout, so
//! this reads LE only — a big-endian path would be dead code that silently
//! invites a wrong call. It also borrows a slice rather than owning a stream:
//! archive entries are already decrypted into memory before parsing, so there
//! is nothing to stream.

use crate::{CwError, Result};

/// A cursor over a byte buffer. All multi-byte reads are little-endian.
pub struct DataReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> DataReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn set_position(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Bytes left to read.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or_else(|| CwError::Invalid {
                what: "DataReader",
                detail: format!("read of {count} bytes overflows the cursor"),
            })?;
        if end > self.data.len() {
            return Err(CwError::Invalid {
                what: "DataReader",
                detail: format!(
                    "read of {count} bytes at {} runs past the end of {} bytes",
                    self.pos,
                    self.data.len()
                ),
            });
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn read_bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        self.take(count)
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn read_i8(&mut self) -> Result<i8> {
        Ok(self.read_u8()? as i8)
    }

    pub fn read_u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_i16(&mut self) -> Result<i16> {
        Ok(self.read_u16()? as i16)
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_i32(&mut self) -> Result<i32> {
        Ok(self.read_u32()? as i32)
    }

    pub fn read_u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_i64(&mut self) -> Result<i64> {
        Ok(self.read_u64()? as i64)
    }

    /// Game structs store 32-bit floats. Never widen these to `f64`.
    pub fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    pub fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.read_u64()?))
    }

    /// Read a NUL-terminated string, as `DataReader.ReadString` does.
    ///
    /// The C# version decodes UTF-8 with the default (lossy) decoder, so
    /// invalid sequences become U+FFFD rather than an error. Names in the game
    /// archives are ASCII in practice; matching the lossy behaviour means a
    /// malformed name yields a readable entry instead of failing a whole
    /// archive.
    pub fn read_string(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0 {
            self.pos += 1;
        }
        let bytes = &self.data[start..self.pos];
        if self.pos < self.data.len() {
            self.pos += 1; // consume the terminator
        }
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_primitives() {
        let data = [0x78, 0x56, 0x34, 0x12, 0xFF, 0xFF];
        let mut r = DataReader::new(&data);
        assert_eq!(r.read_u32().unwrap(), 0x1234_5678);
        assert_eq!(r.read_i16().unwrap(), -1);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn reads_the_rpf_magic_as_the_header_does() {
        // "RPF7" on disk reads back as 0x52504637.
        let data = [0x37, 0x46, 0x50, 0x52];
        assert_eq!(DataReader::new(&data).read_u32().unwrap(), 0x5250_4637);
    }

    #[test]
    fn reads_nul_terminated_strings() {
        let data = b"x64a.rpf\0common.rpf\0";
        let mut r = DataReader::new(data);
        assert_eq!(r.read_string().unwrap(), "x64a.rpf");
        assert_eq!(r.read_string().unwrap(), "common.rpf");
        assert_eq!(r.remaining(), 0);
    }

    /// An unterminated trailing string still yields what is there. The C#
    /// version would throw past the end of the stream; failing an entire
    /// archive over one truncated name is worse than returning the prefix.
    #[test]
    fn reads_an_unterminated_trailing_string() {
        let data = b"truncated";
        assert_eq!(DataReader::new(data).read_string().unwrap(), "truncated");
    }

    #[test]
    fn refuses_to_read_past_the_end() {
        let data = [0u8; 3];
        assert!(DataReader::new(&data).read_u32().is_err());
    }

    #[test]
    fn seeks() {
        let data = [0u8, 1, 2, 3, 4, 5, 6, 7];
        let mut r = DataReader::new(&data);
        r.set_position(4);
        assert_eq!(r.read_u32().unwrap(), 0x0706_0504);
        assert_eq!(r.position(), 8);
    }
}
