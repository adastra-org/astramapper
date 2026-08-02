//! Texture dictionaries: the `.ytd` half of `Texture.cs`.
//!
//! Port of `TextureDictionary` / `TextureBase` / `Texture` / `TextureData` from
//! `CodeWalker.Core/GameFiles/Resources/Texture.cs`, plus the pitch arithmetic
//! `DDSIO.DXTex.ComputePitch` needs to find a mip inside the payload.
//!
//! Scope: the Legacy layout only. The Gen9 branch of `TextureBase.Read` is a
//! different struct at every offset and belongs with the Enhanced edition, which
//! `docs/phases.md` lists as out of scope. `ShaderResourceViewG9` goes with it.
//!
//! Three things about the layout that are not guessable from the struct listing:
//!
//! 1. **`TextureBase` and `Texture` are one struct read in two halves.** The
//!    reference declares the geometry fields (width, height, format, levels,
//!    data pointer) on the *base* class for Gen9's sake, but the Legacy reader
//!    only fills them in the subclass, at offset 0x50. A `TextureBase` that is
//!    not a `Texture` — the shader-parameter case — stops at 80 bytes and has no
//!    pixels at all.
//! 2. **The payload length is not the mip layout.** `TextureData.Read` sizes its
//!    read as `stride * height` halved by four per level, which is the *storage*
//!    length; where each mip actually starts comes from `ComputePitch`, which
//!    clamps a block-compressed mip to one 4x4 block. The two disagree for the
//!    tail mips of any compressed texture, so both are kept: [`Texture::data`]
//!    is read the reference's way and [`Texture::mips`] locates them the other.
//! 3. **A pointer list is read at `capacity`, the items at `count`.** Same rule
//!    as the drawable lists, and the two agree in every stock file.

use serde::Serialize;

use crate::utils::jenk;
use crate::{CwError, Result};

use super::drawable::ResourceFileBase;
use super::reader::ResourceReader;

/// Guard on the texture count before it sizes an allocation. The largest stock
/// `.ytd` holds a few hundred.
const MAX_TEXTURES: usize = 65_536;

/// Guard on a single texture's payload. The largest stock texture is a 2048²
/// uncompressed cube at 16 MB; a corrupt header can name any 32-bit product.
const MAX_TEXTURE_BYTES: usize = 256 * 1024 * 1024;

/// The D3D9 surface format a texture's pixels are in.
///
/// The values are `D3DFORMAT`: small integers for the uncompressed formats and
/// FourCCs for the compressed ones. Only the twelve the reference maps to a
/// DXGI format exist here; anything else in a file is kept as
/// [`Texture::format_raw`] and refused rather than guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TextureFormat {
    A8R8G8B8,
    X8R8G8B8,
    A1R5G5B5,
    A8,
    A8B8G8R8,
    L8,
    Dxt1,
    Dxt3,
    Dxt5,
    Ati1,
    Ati2,
    Bc7,
}

impl TextureFormat {
    pub fn from_raw(value: u32) -> Option<Self> {
        use TextureFormat::*;
        Some(match value {
            21 => A8R8G8B8,
            22 => X8R8G8B8,
            25 => A1R5G5B5,
            28 => A8,
            32 => A8B8G8R8,
            50 => L8,
            0x3154_5844 => Dxt1,
            0x3354_5844 => Dxt3,
            0x3554_5844 => Dxt5,
            0x3149_5441 => Ati1,
            0x3249_5441 => Ati2,
            0x2037_4342 => Bc7,
            _ => return None,
        })
    }

    /// The name the reference's XML and UI use.
    pub fn label(self) -> &'static str {
        use TextureFormat::*;
        match self {
            A8R8G8B8 => "D3DFMT_A8R8G8B8",
            X8R8G8B8 => "D3DFMT_X8R8G8B8",
            A1R5G5B5 => "D3DFMT_A1R5G5B5",
            A8 => "D3DFMT_A8",
            A8B8G8R8 => "D3DFMT_A8B8G8R8",
            L8 => "D3DFMT_L8",
            Dxt1 => "D3DFMT_DXT1",
            Dxt3 => "D3DFMT_DXT3",
            Dxt5 => "D3DFMT_DXT5",
            Ati1 => "D3DFMT_ATI1",
            Ati2 => "D3DFMT_ATI2",
            Bc7 => "D3DFMT_BC7",
        }
    }

    /// Whether the pixels are stored as 4x4 blocks.
    pub fn is_compressed(self) -> bool {
        use TextureFormat::*;
        matches!(self, Dxt1 | Dxt3 | Dxt5 | Ati1 | Ati2 | Bc7)
    }

    /// Bytes per 4x4 block, for the compressed formats.
    pub fn block_bytes(self) -> usize {
        use TextureFormat::*;
        match self {
            Dxt1 | Ati1 => 8,
            Dxt3 | Dxt5 | Ati2 | Bc7 => 16,
            _ => 0,
        }
    }

    /// Bits per pixel, for the uncompressed formats.
    pub fn bits_per_pixel(self) -> usize {
        use TextureFormat::*;
        match self {
            A8R8G8B8 | X8R8G8B8 | A8B8G8R8 => 32,
            A1R5G5B5 => 16,
            A8 | L8 => 8,
            _ => 0,
        }
    }

    /// The row and slice pitch of one mip.
    ///
    /// Port of `DDSIO.DXTex.ComputePitch` restricted to the formats above. Note
    /// the `max(1, ...)`: a 2x2 or 1x1 mip of a compressed texture still costs
    /// one whole block, which is exactly where this disagrees with the
    /// `stride / 4` arithmetic in `TextureData.Read`.
    pub fn compute_pitch(self, width: usize, height: usize) -> (usize, usize) {
        if self.is_compressed() {
            let blocks_x = width.div_ceil(4).max(1);
            let blocks_y = height.div_ceil(4).max(1);
            let row = blocks_x * self.block_bytes();
            return (row, row * blocks_y);
        }
        let row = (width * self.bits_per_pixel()).div_ceil(8);
        (row, row * height)
    }
}

/// What the texture is used for, from the low five bits of `UsageData`.
///
/// The reference enumerates 29 values and marks seven of them unused by V. All
/// are kept: the label is only ever shown, and an unknown value is more useful
/// named than as a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TextureUsage(pub u8);

impl TextureUsage {
    pub fn label(self) -> &'static str {
        match self.0 {
            0 => "unknown",
            1 => "default",
            2 => "terrain",
            3 => "cloud density",
            4 => "cloud normal",
            5 => "cable",
            6 => "fence",
            7 => "env eff",
            8 => "script",
            9 => "water flow",
            10 => "water foam",
            11 => "water fog",
            12 => "water ocean",
            13 => "water",
            14 => "foam opacity",
            15 => "foam",
            16 => "diffuse mip sharpen",
            17 => "diffuse detail",
            18 => "diffuse dark",
            19 => "diffuse alpha opaque",
            20 => "diffuse",
            21 => "detail",
            22 => "normal",
            23 => "specular",
            24 => "emissive",
            25 => "tint palette",
            26 => "skip processing",
            27 => "do not optimize",
            28 => "test",
            _ => "unnamed",
        }
    }
}

/// Where one mip level lives inside [`Texture::data`].
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MipLevel {
    pub level: usize,
    pub width: usize,
    pub height: usize,
    pub row_pitch: usize,
    pub slice_pitch: usize,
    /// Byte offset into the texture's payload.
    pub offset: usize,
}

/// One texture: a surface format, a mip chain, and the bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Texture {
    pub name: Option<String>,
    /// Jenkins hash of the lowercased name, as the reference computes it.
    pub name_hash: u32,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    /// Row pitch of mip 0 as the file declares it. Not recomputed — a file that
    /// disagrees with `ComputePitch` is telling us where its own bytes are.
    pub stride: u16,
    pub format: Option<TextureFormat>,
    /// The `D3DFORMAT` word as stored, kept so an unmapped format can be named.
    pub format_raw: u32,
    pub levels: u8,
    pub usage: TextureUsage,
    /// `UsageData >> 5`. See `TextureUsageFlags` in the reference.
    pub usage_flags: u32,
    pub extra_flags: u32,
    /// The pixel payload, all mips concatenated.
    #[serde(skip)]
    pub data: Vec<u8>,
}

impl Texture {
    /// Read a `Texture` at the cursor: the 80-byte `TextureBase` header, then
    /// the 64 bytes the Legacy subclass adds.
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let _vft = r.read_u32()?;
        let _unknown_4h = r.read_u32()?;
        let _unknown_8h = r.read_u32()?;
        let _unknown_ch = r.read_u32()?;
        let _unknown_10h = r.read_u32()?;
        let _unknown_14h = r.read_u32()?;
        let _unknown_18h = r.read_u32()?;
        let _unknown_1ch = r.read_u32()?;
        let _unknown_20h = r.read_u32()?;
        let _unknown_24h = r.read_u32()?;
        let name_pointer = r.read_u64()?;
        let _unknown_30h = r.read_u16()?;
        let _unknown_32h = r.read_u16()?;
        let _unknown_34h = r.read_u32()?;
        let _unknown_38h = r.read_u32()?;
        let _unknown_3ch = r.read_u32()?;
        let usage_data = r.read_u32()?;
        let _unknown_44h = r.read_u32()?;
        let extra_flags = r.read_u32()?;
        let _unknown_4ch = r.read_u32()?;

        let width = r.read_u16()?;
        let height = r.read_u16()?;
        let depth = r.read_u16()?;
        let stride = r.read_u16()?;
        let format_raw = r.read_u32()?;
        let _unknown_5ch = r.read_u8()?;
        let levels = r.read_u8()?;
        let _unknown_5eh = r.read_u16()?;
        let _unknown_60h = r.read_u32()?;
        let _unknown_64h = r.read_u32()?;
        let _unknown_68h = r.read_u32()?;
        let _unknown_6ch = r.read_u32()?;
        let data_pointer = r.read_u64()?;

        let name = r.read_string_at(name_pointer)?;
        let name_hash = name
            .as_deref()
            .map(|n| jenk::gen_hash(&n.to_lowercase()))
            .unwrap_or(0);

        // `TextureData.Read`, verbatim: stride * height, quartered per level.
        // Not `ComputePitch` — see the module note.
        let mut length = (stride as usize).saturating_mul(height as usize);
        let mut total = 0usize;
        for _ in 0..levels {
            total = total.saturating_add(length);
            length /= 4;
        }
        let data = if total == 0 || total > MAX_TEXTURE_BYTES {
            Vec::new()
        } else {
            r.read_bytes_at(data_pointer, total)?.unwrap_or_default()
        };

        Ok(Self {
            name,
            name_hash,
            width,
            height,
            depth,
            stride,
            format: TextureFormat::from_raw(format_raw),
            format_raw,
            levels,
            usage: TextureUsage((usage_data & 0x1F) as u8),
            usage_flags: usage_data >> 5,
            extra_flags,
            data,
        })
    }

    /// Where each mip starts, as `DDSIO.GetMipmapImages` lays them out.
    ///
    /// Empty when the format is not one we can address. Levels whose bytes run
    /// past the payload are still listed — the offsets are the file's own
    /// layout, and clipping them here would hide a truncated resource from the
    /// caller that has to decide what to do about it.
    pub fn mips(&self) -> Vec<MipLevel> {
        let Some(format) = self.format else {
            return Vec::new();
        };

        let mut out = Vec::with_capacity(self.levels as usize);
        let mut offset = 0usize;
        let mut divisor = 1usize;
        for level in 0..self.levels as usize {
            let width = self.width as usize / divisor;
            let height = self.height as usize / divisor;
            let (row_pitch, slice_pitch) = format.compute_pitch(width, height);
            out.push(MipLevel {
                level,
                width,
                height,
                row_pitch,
                slice_pitch,
                offset,
            });
            offset += slice_pitch;
            divisor *= 2;
        }
        out
    }

    /// The bytes of one mip, or `None` when the level is missing or the payload
    /// is too short to hold it.
    pub fn mip_bytes(&self, level: usize) -> Option<&[u8]> {
        let mip = self.mips().into_iter().find(|m| m.level == level)?;
        self.data.get(mip.offset..mip.offset + mip.slice_pitch)
    }

    /// Whether the pixels can be turned into an image today: a format we map, a
    /// non-zero size and a payload long enough for mip 0.
    pub fn is_decodable(&self) -> bool {
        self.format.is_some()
            && self.width > 0
            && self.height > 0
            && self.levels > 0
            && self.mip_bytes(0).is_some()
    }
}

/// A `.ytd`: textures under Jenkins-hashed names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureDictionary {
    pub base: ResourceFileBase,
    /// Jenkins hashes of the texture names, parallel to `textures`.
    pub hashes: Vec<u32>,
    pub textures: Vec<Texture>,
}

impl TextureDictionary {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        let base = ResourceFileBase::read(r)?;

        let _unknown_10h = r.read_u32()?;
        let _unknown_14h = r.read_u32()?;
        let _unknown_18h = r.read_u32()?;
        let _unknown_1ch = r.read_u32()?;

        // ResourceSimpleList64_uint: read at count.
        let hashes_pointer = r.read_u64()?;
        let hashes_count = r.read_u16()? as usize;
        let _hashes_capacity = r.read_u16()?;
        let _hashes_pad = r.read_u32()?;

        // ResourcePointerList64<Texture>: pointers at capacity, items at count.
        let textures_pointer = r.read_u64()?;
        let textures_count = r.read_u16()? as usize;
        let textures_capacity = r.read_u16()? as usize;
        let _textures_pad = r.read_u32()?;

        // The two arrays are parallel by definition — `BuildDict` pairs them by
        // index — so counts that disagree mean these sixteen bytes are not a
        // dictionary header at all.
        //
        // Six stock files hit this, all of them resources whose payload does
        // not inflate to the size their page flags declare: two extract to zero
        // bytes and the rest to about two thirds. Their headers are therefore
        // read out of whatever the payload does hold. The reference throws on
        // every one of them — `ResourceDataReader`'s constructor slices two
        // `MemoryStream`s out of a buffer too short for them, and the clamping
        // that would avoid it is commented out — so refusing here is the same
        // verdict, reached one layer earlier with the reason attached. See
        // `docs/formats.md` and the gate's `MAX_UNREADABLE_DICTIONARIES`.
        if hashes_count != textures_count {
            return Err(CwError::Invalid {
                what: "texture dictionary",
                detail: format!(
                    "{hashes_count} names for {textures_count} textures; the two lists are \
                     parallel, so this is not a dictionary header — most likely a resource \
                     whose payload is shorter than its page flags declare"
                ),
            });
        }
        if textures_capacity > MAX_TEXTURES {
            return Err(CwError::Invalid {
                what: "texture dictionary",
                detail: format!("{textures_capacity} textures is not a real dictionary"),
            });
        }

        let hashes = r
            .read_u32s_at(hashes_pointer, hashes_count)?
            .unwrap_or_default();

        let pointers = r
            .read_u64s_at(textures_pointer, textures_capacity)?
            .unwrap_or_default();
        let mut textures = Vec::with_capacity(textures_count.min(pointers.len()));
        for pointer in pointers.into_iter().take(textures_count) {
            if let Some(texture) = r.at(pointer, Texture::read)? {
                textures.push(texture);
            }
        }

        Ok(Self {
            base,
            hashes,
            textures,
        })
    }

    /// The texture stored under `hash`, by the hash table rather than by name.
    pub fn lookup(&self, hash: u32) -> Option<&Texture> {
        let index = self.hashes.iter().position(|h| *h == hash)?;
        self.textures.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::reader::SYSTEM_BASE;

    #[test]
    fn maps_the_twelve_formats_the_reference_maps() {
        assert_eq!(TextureFormat::from_raw(21), Some(TextureFormat::A8R8G8B8));
        assert_eq!(TextureFormat::from_raw(50), Some(TextureFormat::L8));
        // The FourCCs really are little-endian ASCII: "DXT1", "ATI2", "BC7 ".
        assert_eq!(
            TextureFormat::from_raw(u32::from_le_bytes(*b"DXT1")),
            Some(TextureFormat::Dxt1)
        );
        assert_eq!(
            TextureFormat::from_raw(u32::from_le_bytes(*b"ATI2")),
            Some(TextureFormat::Ati2)
        );
        assert_eq!(
            TextureFormat::from_raw(u32::from_le_bytes(*b"BC7 ")),
            Some(TextureFormat::Bc7)
        );
        assert_eq!(TextureFormat::from_raw(0), None);
        assert_eq!(TextureFormat::from_raw(77), None);
    }

    /// From `ComputePitch`: BC1 and BC4 are 8 bytes a block, the rest 16, and a
    /// mip smaller than one block still costs a whole one.
    #[test]
    fn computes_the_pitch_of_a_compressed_mip() {
        assert_eq!(TextureFormat::Dxt1.compute_pitch(256, 256), (512, 32_768));
        assert_eq!(TextureFormat::Dxt5.compute_pitch(256, 256), (1024, 65_536));
        assert_eq!(TextureFormat::Dxt1.compute_pitch(4, 4), (8, 8));
        assert_eq!(TextureFormat::Dxt1.compute_pitch(1, 1), (8, 8));
        assert_eq!(TextureFormat::Bc7.compute_pitch(2, 2), (16, 16));
    }

    #[test]
    fn computes_the_pitch_of_an_uncompressed_mip() {
        assert_eq!(TextureFormat::A8R8G8B8.compute_pitch(64, 32), (256, 8192));
        assert_eq!(TextureFormat::A1R5G5B5.compute_pitch(64, 32), (128, 4096));
        assert_eq!(TextureFormat::L8.compute_pitch(64, 32), (64, 2048));
    }

    fn put_u16(d: &mut [u8], at: usize, v: u16) {
        d[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u32(d: &mut [u8], at: usize, v: u32) {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u64(d: &mut [u8], at: usize, v: u64) {
        d[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// One dictionary holding one 8x8 DXT1 texture with two mips, every block
    /// at a known address. Any drifted read lands on a neighbouring field.
    fn synthetic_dictionary() -> Vec<u8> {
        // Layout, all in the system segment:
        //   0x000  TextureDictionary (64)
        //   0x040  name hash array (4)
        //   0x050  texture pointer array (8)
        //   0x060  Texture (144)
        //   0x100  name
        //   0x110  pixels (8x8 DXT1: 32 bytes mip 0 + 8 mip 1)
        let mut d = vec![0u8; 0x200];

        let addr = |offset: u64| SYSTEM_BASE + offset;

        put_u32(&mut d, 0x18, 1); // Unknown_18h, always 1
        put_u64(&mut d, 0x20, addr(0x040)); // name hashes
        put_u16(&mut d, 0x28, 1); // count
        put_u16(&mut d, 0x2A, 1); // capacity
        put_u64(&mut d, 0x30, addr(0x050)); // textures
        put_u16(&mut d, 0x38, 1); // count
        put_u16(&mut d, 0x3A, 1); // capacity

        put_u32(&mut d, 0x40, 0xDEAD_BEEF);
        put_u64(&mut d, 0x50, addr(0x060));

        put_u64(&mut d, 0x60 + 0x28, addr(0x100)); // name pointer
        put_u32(&mut d, 0x60 + 0x40, (3 << 5) | 20); // usage: diffuse, flags 3
        put_u32(&mut d, 0x60 + 0x48, 1); // extra flags
        put_u16(&mut d, 0x60 + 0x50, 8); // width
        put_u16(&mut d, 0x60 + 0x52, 8); // height
        put_u16(&mut d, 0x60 + 0x54, 1); // depth
        put_u16(&mut d, 0x60 + 0x56, 16); // stride: 2 blocks * 8 bytes
        put_u32(&mut d, 0x60 + 0x58, u32::from_le_bytes(*b"DXT1"));
        d[0x60 + 0x5D] = 2; // levels
        put_u64(&mut d, 0x60 + 0x70, addr(0x110)); // data pointer

        d[0x100..0x100 + 10].copy_from_slice(b"grass_diff");
        for (i, byte) in d[0x110..0x110 + 40].iter_mut().enumerate() {
            *byte = i as u8;
        }

        d
    }

    #[test]
    fn reads_a_dictionary_end_to_end() {
        let data = synthetic_dictionary();
        let mut r = ResourceReader::new(&data, data.len());
        let dictionary = TextureDictionary::read(&mut r).unwrap();

        assert_eq!(dictionary.hashes, vec![0xDEAD_BEEF]);
        assert_eq!(dictionary.textures.len(), 1);

        let texture = &dictionary.textures[0];
        assert_eq!(texture.name.as_deref(), Some("grass_diff"));
        assert_eq!(texture.name_hash, jenk::gen_hash("grass_diff"));
        assert_eq!(texture.width, 8);
        assert_eq!(texture.height, 8);
        assert_eq!(texture.stride, 16);
        assert_eq!(texture.format, Some(TextureFormat::Dxt1));
        assert_eq!(texture.levels, 2);
        assert_eq!(texture.usage.0, 20);
        assert_eq!(texture.usage.label(), "diffuse");
        assert_eq!(texture.usage_flags, 3);
        assert_eq!(texture.extra_flags, 1);

        // `TextureData.Read`: 16*8 = 128, then 128/4 = 32. The payload is only
        // 40 real bytes; the reader zero-fills the rest rather than failing.
        assert_eq!(texture.data.len(), 160);
        assert_eq!(&texture.data[..4], &[0, 1, 2, 3]);

        assert!(dictionary.lookup(0xDEAD_BEEF).is_some());
        assert!(dictionary.lookup(1).is_none());
    }

    /// The two mips of an 8x8 DXT1 texture are 32 and 8 bytes, laid out back to
    /// back — which is *not* where `stride * height / 4` would put the second.
    #[test]
    fn locates_the_mips_by_pitch_not_by_the_storage_length() {
        let data = synthetic_dictionary();
        let mut r = ResourceReader::new(&data, data.len());
        let texture = &TextureDictionary::read(&mut r).unwrap().textures[0];

        let mips = texture.mips();
        assert_eq!(mips.len(), 2);
        assert_eq!((mips[0].width, mips[0].height), (8, 8));
        assert_eq!((mips[0].offset, mips[0].slice_pitch), (0, 32));
        assert_eq!((mips[1].width, mips[1].height), (4, 4));
        assert_eq!((mips[1].offset, mips[1].slice_pitch), (32, 8));

        assert_eq!(texture.mip_bytes(0).unwrap()[0], 0);
        assert_eq!(texture.mip_bytes(1).unwrap()[0], 32);
        assert!(texture.mip_bytes(2).is_none());
        assert!(texture.is_decodable());
    }

    /// A format the reference does not map is kept as a number and refused, not
    /// decoded as something else.
    #[test]
    fn refuses_an_unmapped_format() {
        let mut data = synthetic_dictionary();
        put_u32(&mut data, 0x60 + 0x58, 999);
        let mut r = ResourceReader::new(&data, data.len());
        let texture = &TextureDictionary::read(&mut r).unwrap().textures[0];

        assert_eq!(texture.format, None);
        assert_eq!(texture.format_raw, 999);
        assert!(texture.mips().is_empty());
        assert!(!texture.is_decodable());
    }

    /// A junk payload parses into an empty dictionary rather than failing, the
    /// same way a junk drawable does — the reader zero-fills and a null pointer
    /// reads as an absent block.
    #[test]
    fn reads_an_empty_dictionary_from_zeros() {
        let data = vec![0u8; 0x100];
        let mut r = ResourceReader::new(&data, data.len());
        let dictionary = TextureDictionary::read(&mut r).unwrap();
        assert!(dictionary.hashes.is_empty());
        assert!(dictionary.textures.is_empty());
    }

    /// Two counts that disagree are not a dictionary header. A truncated
    /// resource reads its header out of whatever bytes are there, and without
    /// this it would come back as a plausible-looking pile of hashes with no
    /// textures under them.
    #[test]
    fn refuses_a_header_whose_parallel_lists_disagree() {
        let mut data = synthetic_dictionary();
        put_u16(&mut data, 0x28, 9368); // name hash count
        let mut r = ResourceReader::new(&data, data.len());
        let error = TextureDictionary::read(&mut r).unwrap_err().to_string();
        assert!(error.contains("9368 names for 1 textures"), "{error}");
    }
}
