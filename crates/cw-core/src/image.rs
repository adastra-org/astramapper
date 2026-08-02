//! Turning a [`Texture`] into something that can be looked at: RGBA pixels, a
//! PNG, or a `.dds` file.
//!
//! Port of the parts of `CodeWalker.Core/GameFiles/Utils/DDSIO.cs` a viewer
//! needs — `GetPixels` and `GetDDSFile`. Not a port of its BC decoders: those
//! are `bcdec_rs`, which is the reference implementation of the D3D block
//! formats. Two consequences worth knowing before comparing output:
//!
//! 1. **The interpolated block colours differ from CodeWalker's by a unit or
//!    two.** `DecompressDxt1Block` expands the two endpoint colours to 8 bits
//!    and *then* interpolates with truncating integer division; the D3D spec,
//!    and every GPU, interpolates in the packed 5/6-bit domain and rounds. The
//!    endpoints themselves agree exactly — both use the same 565→888 expansion —
//!    so only indices 2 and 3 of a block can differ, and only by rounding. This
//!    port follows the spec. `docs/formats.md` records it and the gate pins the
//!    bound.
//! 2. **BC7 decodes here and does not there.** The reference's `GetPixels` has
//!    an empty `case DXGI_FORMAT_BC7_UNORM: //BC7 TODO!!`, so CodeWalker shows
//!    nothing for a BC7 texture. There is no oracle for that format; the gate
//!    checks it against its own DDS round trip instead.
//!
//! The channel mapping *is* the reference's, including where that is lossy:
//! `D3DFMT_L8` and `D3DFMT_ATI1` carry one channel and land in red alone, not
//! spread across RGB as a grey. Matching what CodeWalker draws matters more here
//! than second-guessing what the texture meant.

use crate::resources::texture::{Texture, TextureFormat};
use crate::{CwError, Result};

/// Decoded pixels, tightly packed RGBA8, top row first.
#[derive(Debug, Clone)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    /// `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// The pixel at `(x, y)`, or `None` outside the image.
    pub fn pixel(&self, x: usize, y: usize) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y * self.width + x) * 4;
        Some([
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ])
    }
}

/// Decode one mip level to RGBA8.
///
/// `level` is clamped into the texture's mip chain, matching `GetPixels`, which
/// clamps rather than refusing an out-of-range mip.
pub fn decode_mip(texture: &Texture, level: usize) -> Result<Rgba> {
    let format = texture.format.ok_or_else(|| CwError::Invalid {
        what: "texture",
        detail: format!("format {:#x} has no decoder", texture.format_raw),
    })?;

    let mips = texture.mips();
    if mips.is_empty() {
        return Err(CwError::Invalid {
            what: "texture",
            detail: format!("{} has no mip levels", name_of(texture)),
        });
    }
    let mip = mips[level.min(mips.len() - 1)];

    if mip.width == 0 || mip.height == 0 {
        return Err(CwError::Invalid {
            what: "texture",
            detail: format!(
                "{} mip {} is {}x{}",
                name_of(texture),
                mip.level,
                mip.width,
                mip.height
            ),
        });
    }

    let bytes = texture
        .data
        .get(mip.offset..mip.offset + mip.slice_pitch)
        .ok_or_else(|| CwError::Invalid {
            what: "texture",
            detail: format!(
                "{} mip {} wants bytes {}..{} of a {}-byte payload",
                name_of(texture),
                mip.level,
                mip.offset,
                mip.offset + mip.slice_pitch,
                texture.data.len()
            ),
        })?;

    let mut pixels = vec![0u8; mip.width * mip.height * 4];
    if format.is_compressed() {
        decode_blocks(format, bytes, mip.width, mip.height, &mut pixels);
    } else {
        decode_linear(
            format,
            bytes,
            mip.width,
            mip.height,
            mip.row_pitch,
            &mut pixels,
        );
    }

    Ok(Rgba {
        width: mip.width,
        height: mip.height,
        pixels,
    })
}

fn name_of(texture: &Texture) -> &str {
    texture.name.as_deref().unwrap_or("this texture")
}

/// Walk the 4x4 blocks, decode each into a scratch tile, and copy the part of
/// the tile that lies inside the image. The edge clip is what makes a
/// non-multiple-of-four texture work.
fn decode_blocks(format: TextureFormat, bytes: &[u8], width: usize, height: usize, out: &mut [u8]) {
    let block_bytes = format.block_bytes();
    let blocks_x = width.div_ceil(4).max(1);
    let blocks_y = height.div_ceil(4).max(1);

    let mut tile = [0u8; 4 * 4 * 4];
    let mut two_channel = [0u8; 4 * 4 * 2];
    let mut one_channel = [0u8; 4 * 4];

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let at = (by * blocks_x + bx) * block_bytes;
            let Some(block) = bytes.get(at..at + block_bytes) else {
                // A payload short of its own block count: leave the rest
                // transparent rather than dropping the mip entirely.
                return;
            };

            match format {
                TextureFormat::Dxt1 => bcdec_rs::bc1(block, &mut tile, 16),
                TextureFormat::Dxt3 => bcdec_rs::bc2(block, &mut tile, 16),
                TextureFormat::Dxt5 => bcdec_rs::bc3(block, &mut tile, 16),
                TextureFormat::Bc7 => bcdec_rs::bc7(block, &mut tile, 16),
                TextureFormat::Ati1 => {
                    bcdec_rs::bc4(block, &mut one_channel, 4, false);
                    // The reference puts the single channel in red and leaves
                    // green and blue at zero.
                    for (i, value) in one_channel.iter().enumerate() {
                        tile[i * 4] = *value;
                        tile[i * 4 + 1] = 0;
                        tile[i * 4 + 2] = 0;
                        tile[i * 4 + 3] = 255;
                    }
                }
                TextureFormat::Ati2 => {
                    bcdec_rs::bc5(block, &mut two_channel, 8, false);
                    for i in 0..16 {
                        tile[i * 4] = two_channel[i * 2];
                        tile[i * 4 + 1] = two_channel[i * 2 + 1];
                        tile[i * 4 + 2] = 0;
                        tile[i * 4 + 3] = 255;
                    }
                }
                _ => return,
            }

            for row in 0..4 {
                let y = by * 4 + row;
                if y >= height {
                    break;
                }
                let columns = 4.min(width - bx * 4);
                let from = row * 16;
                let to = (y * width + bx * 4) * 4;
                out[to..to + columns * 4].copy_from_slice(&tile[from..from + columns * 4]);
            }
        }
    }
}

/// The uncompressed formats, row by row so a declared pitch wider than the
/// pixels is skipped rather than shifting the image.
fn decode_linear(
    format: TextureFormat,
    bytes: &[u8],
    width: usize,
    height: usize,
    row_pitch: usize,
    out: &mut [u8],
) {
    for y in 0..height {
        let Some(row) = bytes.get(y * row_pitch..(y + 1) * row_pitch) else {
            return;
        };
        for x in 0..width {
            let to = (y * width + x) * 4;
            let rgba = match format {
                // Stored BGRA; the reference swaps nothing and lets the bitmap
                // read it as BGRA, so red and blue swap here instead.
                TextureFormat::A8R8G8B8 => {
                    let p = &row[x * 4..x * 4 + 4];
                    [p[2], p[1], p[0], p[3]]
                }
                TextureFormat::X8R8G8B8 => {
                    let p = &row[x * 4..x * 4 + 4];
                    [p[2], p[1], p[0], 255]
                }
                TextureFormat::A8B8G8R8 => {
                    let p = &row[x * 4..x * 4 + 4];
                    [p[0], p[1], p[2], p[3]]
                }
                // One byte of alpha shown as grey, as `ConvertA8ToRGBA8` does.
                TextureFormat::A8 => {
                    let v = row[x];
                    [v, v, v, 255]
                }
                // One byte of luminance shown as red, as `ConvertR8ToRGBA8`
                // does. See the module note.
                TextureFormat::L8 => [row[x], 0, 0, 255],
                TextureFormat::A1R5G5B5 => {
                    let v = u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]);
                    let r = ((v >> 10) & 0x1F) as u32;
                    let g = ((v >> 5) & 0x1F) as u32;
                    let b = (v & 0x1F) as u32;
                    [
                        ((r * 527 + 23) >> 6) as u8,
                        ((g * 527 + 23) >> 6) as u8,
                        ((b * 527 + 23) >> 6) as u8,
                        if v & 0x8000 != 0 { 255 } else { 0 },
                    ]
                }
                _ => return,
            };
            out[to..to + 4].copy_from_slice(&rgba);
        }
    }
}

/// Encode RGBA pixels as a PNG.
///
/// PNG rather than raw pixels because this crosses the IPC boundary: a 2048²
/// texture is 16 MB of RGBA and 22 MB once base64'd, which is slower to
/// serialize than to decode.
pub fn to_png(image: &Rgba) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width as u32, image.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| png_error("write the PNG header", e))?;
        writer
            .write_image_data(&image.pixels)
            .map_err(|e| png_error("write the PNG pixels", e))?;
        writer
            .finish()
            .map_err(|e| png_error("finish the PNG", e))?;
    }
    Ok(out)
}

fn png_error(what: &'static str, error: png::EncodingError) -> CwError {
    CwError::Invalid {
        what: "png",
        detail: format!("could not {what}: {error}"),
    }
}

/// The `DDS_PIXELFORMAT` for a format, or `None` when it needs the DX10
/// extension header instead.
///
/// From `DDSIO.DXTex._EncodeDDSHeader`. Returns `(flags, four_cc, bit_count,
/// r_mask, g_mask, b_mask, a_mask)`.
fn pixel_format(format: TextureFormat) -> Option<(u32, u32, u32, u32, u32, u32, u32)> {
    const FOURCC: u32 = 0x0000_0004;
    const RGB: u32 = 0x0000_0040;
    const RGBA: u32 = 0x0000_0041;
    const LUMINANCE: u32 = 0x0002_0000;
    const ALPHA: u32 = 0x0000_0002;

    let four_cc = |s: &[u8; 4]| u32::from_le_bytes(*s);

    use TextureFormat::*;
    Some(match format {
        Dxt1 => (FOURCC, four_cc(b"DXT1"), 0, 0, 0, 0, 0),
        Dxt3 => (FOURCC, four_cc(b"DXT3"), 0, 0, 0, 0, 0),
        Dxt5 => (FOURCC, four_cc(b"DXT5"), 0, 0, 0, 0, 0),
        // The reference writes the DirectXTex spelling, not the D3D9 "ATI1".
        Ati1 => (FOURCC, four_cc(b"BC4U"), 0, 0, 0, 0, 0),
        Ati2 => (FOURCC, four_cc(b"BC5U"), 0, 0, 0, 0, 0),
        A8R8G8B8 => (RGBA, 0, 32, 0x00FF_0000, 0xFF00, 0xFF, 0xFF00_0000),
        X8R8G8B8 => (RGB, 0, 32, 0x00FF_0000, 0xFF00, 0xFF, 0),
        A8B8G8R8 => (RGBA, 0, 32, 0xFF, 0xFF00, 0x00FF_0000, 0xFF00_0000),
        A1R5G5B5 => (RGBA, 0, 16, 0x7C00, 0x03E0, 0x1F, 0x8000),
        L8 => (LUMINANCE, 0, 8, 0xFF, 0, 0, 0),
        A8 => (ALPHA, 0, 8, 0, 0, 0, 0xFF),
        // BC7 has no legacy FourCC, so DirectXTex falls through to DX10.
        Bc7 => return None,
    })
}

/// The `DXGI_FORMAT` a texture is written as in a DX10 extension header.
fn dxgi_format(format: TextureFormat) -> u32 {
    use TextureFormat::*;
    match format {
        Dxt1 => 71,     // BC1_UNORM
        Dxt3 => 74,     // BC2_UNORM
        Dxt5 => 77,     // BC3_UNORM
        Ati1 => 80,     // BC4_UNORM
        Ati2 => 83,     // BC5_UNORM
        Bc7 => 98,      // BC7_UNORM
        A8R8G8B8 => 87, // B8G8R8A8_UNORM
        X8R8G8B8 => 88, // B8G8R8X8_UNORM
        A8B8G8R8 => 28, // R8G8B8A8_UNORM
        A1R5G5B5 => 86, // B5G5R5A1_UNORM
        L8 => 61,       // R8_UNORM
        A8 => 65,       // A8_UNORM
    }
}

/// Write a texture out as a `.dds` file.
///
/// Byte for byte what `DDSIO.GetDDSFile` produces: the magic, a 124-byte
/// `DDS_HEADER`, a 20-byte `DDS_HEADER_DXT10` when the format has no legacy
/// pixel format, then every mip laid out back to back at its computed pitch.
pub fn to_dds(texture: &Texture) -> Result<Vec<u8>> {
    let format = texture.format.ok_or_else(|| CwError::Invalid {
        what: "texture",
        detail: format!("format {:#x} cannot be written as DDS", texture.format_raw),
    })?;

    const HEADER_FLAGS_TEXTURE: u32 = 0x0000_1007;
    const HEADER_FLAGS_MIPMAP: u32 = 0x0002_0000;
    const HEADER_FLAGS_PITCH: u32 = 0x0000_0008;
    const HEADER_FLAGS_LINEARSIZE: u32 = 0x0008_0000;
    const SURFACE_FLAGS_TEXTURE: u32 = 0x0000_1000;
    const SURFACE_FLAGS_MIPMAP: u32 = 0x0040_0008;

    let width = texture.width as usize;
    let height = texture.height as usize;
    let levels = texture.levels.max(1) as u32;
    let (row_pitch, slice_pitch) = format.compute_pitch(width, height);

    let mut flags = HEADER_FLAGS_TEXTURE | HEADER_FLAGS_MIPMAP;
    let mut caps = SURFACE_FLAGS_TEXTURE;
    if levels > 1 {
        caps |= SURFACE_FLAGS_MIPMAP;
    }
    let pitch_or_linear_size = if format.is_compressed() {
        flags |= HEADER_FLAGS_LINEARSIZE;
        slice_pitch as u32
    } else {
        flags |= HEADER_FLAGS_PITCH;
        row_pitch as u32
    };

    let mut out = Vec::with_capacity(148 + texture.data.len());
    let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());

    put(0x2053_4444); // "DDS "
    put(124); // dwSize
    put(flags);
    put(height as u32);
    put(width as u32);
    put(pitch_or_linear_size);
    put(1); // dwDepth
    put(levels);
    for _ in 0..11 {
        put(0); // dwReserved1
    }

    let legacy = pixel_format(format);
    put(32); // ddspf.dwSize
    match legacy {
        Some((pf_flags, four_cc, bits, r, g, b, a)) => {
            put(pf_flags);
            put(four_cc);
            put(bits);
            put(r);
            put(g);
            put(b);
            put(a);
        }
        None => {
            put(0x0000_0004); // DDS_FOURCC
            put(u32::from_le_bytes(*b"DX10"));
            put(0);
            put(0);
            put(0);
            put(0);
            put(0);
        }
    }

    put(caps);
    put(0); // dwCaps2
    put(0); // dwCaps3
    put(0); // dwCaps4
    put(0); // dwReserved2

    if legacy.is_none() {
        put(dxgi_format(format));
        put(3); // TEX_DIMENSION_TEXTURE2D
        put(0); // miscFlag
        put(1); // arraySize
        put(0); // miscFlags2
    }

    // The reference clips the last mip to whatever payload is left rather than
    // failing, and writes nothing once the payload runs out.
    for mip in texture.mips() {
        if mip.offset >= texture.data.len() {
            break;
        }
        let end = (mip.offset + mip.slice_pitch).min(texture.data.len());
        out.extend_from_slice(&texture.data[mip.offset..end]);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A texture built in memory rather than read from a file, so the decoders
    /// can be tested without a fixture.
    fn texture(format: TextureFormat, width: u16, height: u16, data: Vec<u8>) -> Texture {
        let (row_pitch, _) = format.compute_pitch(width as usize, height as usize);
        Texture {
            name: Some("test".into()),
            name_hash: 0,
            width,
            height,
            depth: 1,
            stride: row_pitch as u16,
            format: Some(format),
            format_raw: 0,
            levels: 1,
            usage: crate::resources::texture::TextureUsage(0),
            usage_flags: 0,
            extra_flags: 0,
            data,
        }
    }

    /// A DXT1 block whose two endpoints are pure red and pure blue, with all
    /// sixteen pixels on endpoint 0. The endpoint expansion is the one place
    /// this port and CodeWalker agree exactly, so it is what gets pinned.
    #[test]
    fn decodes_a_dxt1_block() {
        let red = 0xF800u16; // 565: r=31
        let blue = 0x001Fu16; // 565: b=31
        let mut block = Vec::new();
        block.extend_from_slice(&red.to_le_bytes());
        block.extend_from_slice(&blue.to_le_bytes());
        block.extend_from_slice(&0u32.to_le_bytes()); // every index 0

        let image = decode_mip(&texture(TextureFormat::Dxt1, 4, 4, block), 0).unwrap();
        assert_eq!((image.width, image.height), (4, 4));
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(image.pixel(x, y), Some([255, 0, 0, 255]), "at {x},{y}");
            }
        }
    }

    /// Index 3 of a block whose first endpoint is the smaller is transparent
    /// black — the BC1A punch-through mode, and the only way a DXT1 texture
    /// carries alpha.
    #[test]
    fn decodes_the_dxt1_punch_through_mode() {
        let mut block = Vec::new();
        block.extend_from_slice(&0x001Fu16.to_le_bytes()); // c0 < c1
        block.extend_from_slice(&0xF800u16.to_le_bytes());
        block.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // every index 3

        let image = decode_mip(&texture(TextureFormat::Dxt1, 4, 4, block), 0).unwrap();
        assert_eq!(image.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    /// A texture whose width is not a multiple of four still decodes: the block
    /// grid is rounded up and the tile clipped on copy.
    #[test]
    fn clips_a_block_grid_to_a_ragged_edge() {
        let mut block = Vec::new();
        block.extend_from_slice(&0xF800u16.to_le_bytes());
        block.extend_from_slice(&0xF800u16.to_le_bytes());
        block.extend_from_slice(&0u32.to_le_bytes());

        let image = decode_mip(&texture(TextureFormat::Dxt1, 3, 2, block), 0).unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(image.pixels.len(), 3 * 2 * 4);
        assert_eq!(image.pixel(2, 1), Some([255, 0, 0, 255]));
    }

    /// `D3DFMT_A8R8G8B8` is stored BGRA. Getting this backwards is invisible on
    /// a grey texture and obvious on everything else.
    #[test]
    fn swaps_red_and_blue_for_the_bgra_formats() {
        let bgra = vec![10u8, 20, 30, 40];
        let image = decode_mip(&texture(TextureFormat::A8R8G8B8, 1, 1, bgra), 0).unwrap();
        assert_eq!(image.pixel(0, 0), Some([30, 20, 10, 40]));

        let rgba = vec![10u8, 20, 30, 40];
        let image = decode_mip(&texture(TextureFormat::A8B8G8R8, 1, 1, rgba), 0).unwrap();
        assert_eq!(image.pixel(0, 0), Some([10, 20, 30, 40]));
    }

    /// `X8R8G8B8` has no alpha channel, so the stored byte is ignored rather
    /// than shown.
    #[test]
    fn forces_alpha_opaque_for_the_x_formats() {
        let image =
            decode_mip(&texture(TextureFormat::X8R8G8B8, 1, 1, vec![1, 2, 3, 0]), 0).unwrap();
        assert_eq!(image.pixel(0, 0), Some([3, 2, 1, 255]));
    }

    /// The single-channel formats follow the reference: alpha becomes grey,
    /// luminance becomes red. See the module note.
    #[test]
    fn maps_the_single_channel_formats_the_way_the_reference_does() {
        let image = decode_mip(&texture(TextureFormat::A8, 1, 1, vec![77]), 0).unwrap();
        assert_eq!(image.pixel(0, 0), Some([77, 77, 77, 255]));

        let image = decode_mip(&texture(TextureFormat::L8, 1, 1, vec![77]), 0).unwrap();
        assert_eq!(image.pixel(0, 0), Some([77, 0, 0, 255]));
    }

    /// A1R5G5B5's alpha is one bit: set or clear, never in between.
    #[test]
    fn decodes_the_one_bit_alpha_format() {
        let opaque = 0x8000u16 | (31 << 10); // alpha set, red full
        let image = decode_mip(
            &texture(TextureFormat::A1R5G5B5, 1, 1, opaque.to_le_bytes().to_vec()),
            0,
        )
        .unwrap();
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));

        let clear = 31u16 << 10;
        let image = decode_mip(
            &texture(TextureFormat::A1R5G5B5, 1, 1, clear.to_le_bytes().to_vec()),
            0,
        )
        .unwrap();
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 0]));
    }

    /// A format with no decoder is refused with its raw value named, not
    /// silently drawn as something else.
    #[test]
    fn refuses_a_texture_with_no_format() {
        let mut t = texture(TextureFormat::Dxt1, 4, 4, vec![0; 8]);
        t.format = None;
        t.format_raw = 999;
        let error = decode_mip(&t, 0).unwrap_err().to_string();
        assert!(error.contains("3e7"), "unhelpful error: {error}");
    }

    /// A payload too short for the mip it claims is an error rather than a
    /// half-drawn image.
    #[test]
    fn refuses_a_mip_that_is_not_in_the_payload() {
        let t = texture(TextureFormat::Dxt1, 64, 64, vec![0; 16]);
        assert!(decode_mip(&t, 0).is_err());
    }

    /// `decode_mip` clamps rather than failing, as `GetPixels` does.
    #[test]
    fn clamps_a_mip_level_past_the_end_of_the_chain() {
        let block = vec![0u8; 8];
        let image = decode_mip(&texture(TextureFormat::Dxt1, 4, 4, block), 99).unwrap();
        assert_eq!((image.width, image.height), (4, 4));
    }

    #[test]
    fn encodes_a_png_that_round_trips() {
        let image = Rgba {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 0, 0, 255, 128],
        };
        let png = to_png(&image).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

        let decoder = png::Decoder::new(std::io::Cursor::new(&png));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&buffer[..info.buffer_size()], &image.pixels[..]);
    }

    /// A legacy-format DDS is 128 bytes of header and then the pixels.
    #[test]
    fn writes_a_legacy_dds_header() {
        let mut t = texture(TextureFormat::Dxt1, 8, 8, vec![7u8; 32]);
        t.levels = 1;
        let dds = to_dds(&t).unwrap();

        assert_eq!(&dds[..4], b"DDS ");
        assert_eq!(u32::from_le_bytes(dds[4..8].try_into().unwrap()), 124);
        assert_eq!(u32::from_le_bytes(dds[12..16].try_into().unwrap()), 8); // height
        assert_eq!(u32::from_le_bytes(dds[16..20].try_into().unwrap()), 8); // width
        assert_eq!(u32::from_le_bytes(dds[20..24].try_into().unwrap()), 32); // linear size
        assert_eq!(&dds[84..88], b"DXT1");
        assert_eq!(dds.len(), 128 + 32);
        assert_eq!(&dds[128..], &[7u8; 32]);
    }

    /// BC7 has no legacy FourCC, so it goes out with the DX10 extension header
    /// and the file is 20 bytes longer.
    #[test]
    fn writes_a_dx10_header_for_bc7() {
        let t = texture(TextureFormat::Bc7, 4, 4, vec![0u8; 16]);
        let dds = to_dds(&t).unwrap();

        assert_eq!(&dds[84..88], b"DX10");
        assert_eq!(u32::from_le_bytes(dds[128..132].try_into().unwrap()), 98);
        assert_eq!(u32::from_le_bytes(dds[132..136].try_into().unwrap()), 3);
        assert_eq!(dds.len(), 148 + 16);
    }
}
