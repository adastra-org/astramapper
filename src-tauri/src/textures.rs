//! Handing textures to the GPU in the format it already wants.
//!
//! The world view used to receive every texture as a base64 PNG: decode the BC
//! blocks to full-resolution RGBA, deflate that into a PNG, base64 the PNG,
//! then JSON-escape the base64 — and on the other side the browser inflates the
//! PNG and the driver re-compresses nothing, so the GPU holds four times the
//! VRAM it needed. Measured over 600 real textures
//! (`cargo run --release --example texture_cost`):
//!
//! ```text
//! decode + png + base64 : 12.92s   ->  117.8 MB on the wire
//! raw mip chain         : 19.58ms  ->   94.6 MB on the wire
//! ```
//!
//! 660x on CPU, and *smaller* on the wire despite the raw chain carrying every
//! mip against the PNG path's mip 0 alone. It also allocated 443.9 MB of
//! intermediate RGBA per run that was immediately thrown away.
//!
//! So: ship the blocks. `MipLevel` already records where each level lives
//! inside the payload, so there is no new parsing here at all — this module is
//! offsets, a format name, and the decision of when the blocks will not do.
//!
//! **This is a deliberate change to a rule `docs/architecture.md` states**, that
//! the WebView only ever uploads image bytes and never BC blocks. The reasoning
//! behind the rule was that the browser has no business *parsing* archive
//! formats, and it still does not: the vertex declaration, the mip layout, the
//! dictionary chain and the format identification all resolve here. BC blocks
//! are not an archive format, they are the GPU's own storage format, and the
//! WebView's entire involvement is one `compressedTexImage2D` call per level.
//!
//! When the GPU cannot take a format — an old driver with no BPTC, or one of
//! the uncompressed D3D formats that has no compressed equivalent — this falls
//! back to RGBA, which is still not a PNG. See [`GpuCaps`].

use cw_core::image;
use cw_core::resources::{Texture, TextureFormat};
use serde::{Deserialize, Serialize};

/// What the WebView's GL context can accept, as it reported at world-mode
/// start.
///
/// Defaults to **nothing supported**, so a frontend that never calls
/// `set_gpu_capabilities` gets correct pixels rather than blocks it cannot
/// upload. The handshake is one call; a missing one is a bug, not a policy.
///
/// This is part of the cache key in stage 4: the same texture compiles to
/// different bytes on a machine without BPTC.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuCaps {
    /// `WEBGL_compressed_texture_s3tc` — BC1/2/3. Universal on desktop.
    pub s3tc: bool,
    /// `EXT_texture_compression_rgtc` — BC4/BC5, which GTA uses heavily for
    /// normal maps.
    pub rgtc: bool,
    /// `EXT_texture_compression_bptc` — BC7.
    pub bptc: bool,
}

impl GpuCaps {
    /// A stable 32-bit summary, for the cache key.
    pub fn bits(self) -> u32 {
        (self.s3tc as u32) | ((self.rgtc as u32) << 1) | ((self.bptc as u32) << 2)
    }

    /// Whether blocks in `format` can be uploaded as they are stored.
    fn accepts(self, format: TextureFormat) -> bool {
        use TextureFormat::*;
        match format {
            Dxt1 | Dxt3 | Dxt5 => self.s3tc,
            Ati1 | Ati2 => self.rgtc,
            Bc7 => self.bptc,
            _ => false,
        }
    }
}

/// The name the frontend maps to a WebGL internal format.
///
/// Deliberately the BC names rather than the D3D ones: `TextureFormat::label`
/// already gives `D3DFMT_DXT1` for the Explorer's file inspector, and the
/// frontend is talking to WebGL, where the same thing is called BC1. Keeping
/// the two vocabularies apart stops one being quietly used for the other.
fn block_name(format: TextureFormat) -> &'static str {
    use TextureFormat::*;
    match format {
        Dxt1 => "bc1",
        Dxt3 => "bc2",
        Dxt5 => "bc3",
        Ati1 => "bc4",
        Ati2 => "bc5",
        Bc7 => "bc7",
        _ => "rgba8",
    }
}

/// One mip's dimensions and byte length inside the texture's section.
///
/// The section holds the whole chain concatenated, largest first; the frontend
/// walks these lengths to `subarray` each level. One section per texture rather
/// than one per mip because a `Uint8Array` view has no alignment requirement,
/// so splitting them would buy nothing and cost a table entry each.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireMip {
    pub width: usize,
    pub height: usize,
    pub byte_length: usize,
}

/// One texture as the world view receives it: metadata here, pixels in a
/// section.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTexture {
    /// The Jenkins hash the parts reference — `MeshPart::diffuse` and friends.
    pub hash: u32,
    pub name: String,
    pub width: usize,
    pub height: usize,
    /// `bc1`..`bc7` for blocks, `rgba8` for the decoded fallback.
    pub format: &'static str,
    /// True when `format` is a block format, so the frontend knows to call
    /// `compressedTexImage2D` rather than build a `DataTexture`. Derivable from
    /// the name, but the frontend should not be parsing names to decide which
    /// GL call to make.
    pub compressed: bool,
    /// The chain, largest first. Never empty.
    pub mips: Vec<WireMip>,
    /// Index of the section holding the concatenated chain.
    pub section: usize,
    /// The `.ytd` this was resolved from, or `None` for a drawable's own
    /// embedded dictionary.
    pub source: Option<String>,
}

/// The pixels for one texture, and the metadata describing them.
pub struct EncodedTexture {
    pub meta: WireTexture,
    pub bytes: Vec<u8>,
}

/// Pack one texture for the wire, as blocks when the GPU takes them.
///
/// Returns `None` for a texture that cannot be turned into pixels at all: an
/// unmapped `D3DFORMAT`, a zero dimension, or a payload too short to hold mip
/// 0. That is a texture the PNG path dropped silently too.
///
/// `section` is the index the caller has reserved; this function does not own
/// the envelope, so the caller assigns and the metadata records.
pub fn encode(
    hash: u32,
    texture: &Texture,
    source: Option<&str>,
    caps: GpuCaps,
    section: usize,
) -> Option<EncodedTexture> {
    if !texture.is_decodable() {
        return None;
    }
    let format = texture.format?;
    let levels = texture.mips();
    if levels.is_empty() {
        return None;
    }

    let name = texture
        .name
        .clone()
        .unwrap_or_else(|| format!("{:08x}", texture.name_hash));

    let mut mips = Vec::with_capacity(levels.len());
    let mut bytes: Vec<u8> = Vec::new();

    if format.is_compressed() && caps.accepts(format) {
        // The fast path, and the reason this module exists: a memcpy per level.
        for level in &levels {
            let Some(slice) = texture.mip_bytes(level.level) else {
                // A truncated chain is not a broken texture — everything read so
                // far is uploadable, and the frontend clamps to what it got.
                break;
            };
            mips.push(WireMip {
                width: level.width,
                height: level.height,
                byte_length: slice.len(),
            });
            bytes.extend_from_slice(slice);
        }
    } else {
        // No BC support for this format, or an uncompressed D3D format that has
        // no block equivalent. Still not a PNG: RGBA straight into a
        // `DataTexture` skips the deflate, the base64 and the browser's decoder.
        for level in &levels {
            let Ok(decoded) = image::decode_mip(texture, level.level) else {
                break;
            };
            mips.push(WireMip {
                width: decoded.width,
                height: decoded.height,
                byte_length: decoded.pixels.len(),
            });
            bytes.extend_from_slice(&decoded.pixels);
        }
    }

    if mips.is_empty() {
        return None;
    }
    let compressed = format.is_compressed() && caps.accepts(format);

    Some(EncodedTexture {
        meta: WireTexture {
            hash,
            name,
            width: texture.width as usize,
            height: texture.height as usize,
            format: if compressed {
                block_name(format)
            } else {
                "rgba8"
            },
            compressed,
            mips,
            section,
            source: source.map(str::to_string),
        },
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 8x8 DXT1 texture with a two-level chain: 8x8 is 2x2 blocks at 8 bytes
    /// each, 4x4 is one block.
    fn dxt1_8x8() -> Texture {
        let mut data = vec![0u8; 32 + 8];
        data[0] = 0xaa;
        data[32] = 0xbb;
        Texture {
            name: Some("test_d".into()),
            name_hash: 0x1234_5678,
            width: 8,
            height: 8,
            depth: 1,
            stride: 16,
            format: Some(TextureFormat::Dxt1),
            format_raw: 0x3154_5844,
            levels: 2,
            usage: cw_core::resources::TextureUsage(20),
            usage_flags: 0,
            extra_flags: 0,
            data,
        }
    }

    #[test]
    fn ships_blocks_when_the_gpu_takes_them() {
        let caps = GpuCaps {
            s3tc: true,
            ..Default::default()
        };
        let out = encode(7, &dxt1_8x8(), Some(r"x64a.rpf\a.ytd"), caps, 3).expect("encoded");
        assert_eq!(out.meta.format, "bc1");
        assert!(out.meta.compressed);
        assert_eq!(out.meta.hash, 7);
        assert_eq!(out.meta.section, 3);
        assert_eq!(out.meta.source.as_deref(), Some(r"x64a.rpf\a.ytd"));
        assert_eq!(out.meta.mips.len(), 2);
        assert_eq!((out.meta.mips[0].width, out.meta.mips[0].height), (8, 8));
        assert_eq!(out.meta.mips[0].byte_length, 32);
        assert_eq!(out.meta.mips[1].byte_length, 8);
        // The blocks are handed over untouched, in chain order.
        assert_eq!(out.bytes.len(), 40);
        assert_eq!(out.bytes[0], 0xaa);
        assert_eq!(out.bytes[32], 0xbb);
    }

    /// The metadata's byte lengths must add up to the section, or the frontend
    /// walks off the end of the chain.
    #[test]
    fn the_mip_lengths_account_for_every_byte() {
        let caps = GpuCaps {
            s3tc: true,
            ..Default::default()
        };
        let out = encode(0, &dxt1_8x8(), None, caps, 0).expect("encoded");
        let summed: usize = out.meta.mips.iter().map(|m| m.byte_length).sum();
        assert_eq!(summed, out.bytes.len());
    }

    /// The whole point of the handshake: without the extension the same texture
    /// must arrive as pixels the context can actually upload.
    #[test]
    fn falls_back_to_rgba_without_the_extension() {
        let out = encode(0, &dxt1_8x8(), None, GpuCaps::default(), 0).expect("encoded");
        assert_eq!(out.meta.format, "rgba8");
        assert!(!out.meta.compressed);
        assert_eq!(out.meta.mips[0].byte_length, 8 * 8 * 4);
    }

    #[test]
    fn refuses_a_format_with_no_decoder() {
        let mut texture = dxt1_8x8();
        texture.format = None;
        texture.format_raw = 0xdead;
        let caps = GpuCaps {
            s3tc: true,
            ..Default::default()
        };
        assert!(encode(0, &texture, None, caps, 0).is_none());
    }

    #[test]
    fn names_an_unnamed_texture_by_its_hash() {
        let mut texture = dxt1_8x8();
        texture.name = None;
        let caps = GpuCaps {
            s3tc: true,
            ..Default::default()
        };
        let out = encode(0, &texture, None, caps, 0).expect("encoded");
        assert_eq!(out.meta.name, "12345678");
    }

    /// Each extension gates only its own formats, so a context with S3TC but no
    /// BPTC still gets blocks for DXT and pixels for BC7.
    #[test]
    fn each_extension_gates_only_its_own_formats() {
        let s3tc_only = GpuCaps {
            s3tc: true,
            ..Default::default()
        };
        assert!(s3tc_only.accepts(TextureFormat::Dxt5));
        assert!(!s3tc_only.accepts(TextureFormat::Ati2));
        assert!(!s3tc_only.accepts(TextureFormat::Bc7));

        let all = GpuCaps {
            s3tc: true,
            rgtc: true,
            bptc: true,
        };
        assert!(all.accepts(TextureFormat::Ati1));
        assert!(all.accepts(TextureFormat::Bc7));
        // An uncompressed format has no block form to accept.
        assert!(!all.accepts(TextureFormat::A8R8G8B8));
    }

    #[test]
    fn the_caps_bitset_is_stable() {
        assert_eq!(GpuCaps::default().bits(), 0);
        assert_eq!(
            GpuCaps {
                s3tc: true,
                rgtc: true,
                bptc: true
            }
            .bits(),
            7
        );
        assert_eq!(
            GpuCaps {
                s3tc: false,
                rgtc: false,
                bptc: true
            }
            .bits(),
            4
        );
    }
}
