//! AWC: the audio wave container.
//!
//! Port of the reading half of `CodeWalker.Core/GameFiles/FileTypes/AwcFile.cs`
//! (113 KB), narrowed to what it takes to get playable audio out: the header,
//! the stream and chunk tables, the `format`, `streamformat`, `data` and `mid`
//! chunks, and the multi-channel de-interleave.
//!
//! Not ported: the `peak`, `animation`, `gesture`, `markers`, `granulargrains`,
//! `granularloops` and `seektable` chunks. Their offsets and sizes are kept so
//! the UI can list them; their contents are seek points and metadata for the
//! audio engine, and playing a stream from the start needs none of it.
//!
//! Four things about the format that are not guessable from the struct listing:
//!
//! 1. **A file can be encrypted whole, per block, or not at all**, under a
//!    modified XXTEA. Which one is a flag, and the whole-file case is detected
//!    by the magic failing to match — there is no flag for it.
//! 2. **A "stream" is one channel, not one sound.** A stereo sound is two
//!    streams, paired by a `_left`/`_right` naming convention that lives in the
//!    name tables, not in the file.
//! 3. **In a multi-channel file the audio is in one stream and the others point
//!    into it.** That carrier stream has id 0, holds the `data` and
//!    `streamformat` chunks, and is not itself playable. Its data is a series
//!    of fixed-size blocks, each with its own per-channel header, and a
//!    channel's audio is the concatenation of its slice of every block.
//! 4. **Chunk offsets are absolute in the file**, not relative to the stream.

use serde::Serialize;

use crate::utils::DataReader;
use crate::{CwError, Result};

use super::{adpcm, wav};

/// `ADAT`, little-endian.
pub const MAGIC_LE: u32 = 0x5441_4441;
/// `ADAT` byte-swapped: a big-endian console file.
pub const MAGIC_BE: u32 = 0x4144_4154;

/// XXTEA operates on 4-byte words, and every block in a multi-channel file is
/// this long.
const BLOCK_ALIGN: usize = 0x800;

/// How a stream's samples are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Codec {
    /// Signed 16-bit little-endian.
    Pcm,
    /// IMA ADPCM in 2048-byte blocks. See [`super::adpcm`].
    Adpcm,
    /// The reference names only 0 and 4. Anything else is carried through so a
    /// modded file says what it claims rather than failing to open.
    Unknown(u8),
}

impl Codec {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Codec::Pcm,
            4 => Codec::Adpcm,
            other => Codec::Unknown(other),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Codec::Pcm => "PCM",
            Codec::Adpcm => "ADPCM",
            Codec::Unknown(_) => "unknown",
        }
    }
}

/// The chunk kinds. The value is the last byte of the Jenkins hash of the
/// chunk's name, which is why they look arbitrary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChunkType {
    Data,
    Format,
    Animation,
    Peak,
    Midi,
    Gesture,
    GranularGrains,
    GranularLoops,
    Markers,
    StreamFormat,
    SeekTable,
    Unknown(u8),
}

impl ChunkType {
    fn from_u8(value: u8) -> Self {
        match value {
            0x55 => ChunkType::Data,
            0xFA => ChunkType::Format,
            0x5C => ChunkType::Animation,
            0x36 => ChunkType::Peak,
            0x68 => ChunkType::Midi,
            0x2B => ChunkType::Gesture,
            0x5A => ChunkType::GranularGrains,
            0xD9 => ChunkType::GranularLoops,
            0xBD => ChunkType::Markers,
            0x48 => ChunkType::StreamFormat,
            0xA3 => ChunkType::SeekTable,
            other => ChunkType::Unknown(other),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChunkType::Data => "data",
            ChunkType::Format => "format",
            ChunkType::Animation => "animation",
            ChunkType::Peak => "peak",
            ChunkType::Midi => "mid",
            ChunkType::Gesture => "gesture",
            ChunkType::GranularGrains => "granulargrains",
            ChunkType::GranularLoops => "granularloops",
            ChunkType::Markers => "markers",
            ChunkType::StreamFormat => "streamformat",
            ChunkType::SeekTable => "seektable",
            ChunkType::Unknown(_) => "unknown",
        }
    }
}

/// One entry in a stream's chunk table.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkInfo {
    pub kind: ChunkType,
    pub size: usize,
    /// Absolute offset in the file.
    pub offset: usize,
}

/// A stream's audio format, from either a `format` chunk or one entry of a
/// `streamformat` chunk.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Format {
    pub samples: u32,
    pub sample_rate: u16,
    pub codec: Codec,
    /// Attenuation the audio engine applies, in units of 100 dB.
    pub headroom: i16,
}

impl Format {
    /// Duration in seconds, or zero for a stream with no sample rate.
    pub fn seconds(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples as f32 / self.sample_rate as f32
    }
}

/// One channel of audio, or the carrier stream that holds several.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    /// The 29-bit name hash. Zero for a multi-channel carrier.
    pub id: u32,
    pub chunks: Vec<ChunkInfo>,
    pub format: Option<Format>,
    /// True for the stream that carries the audio for all the others.
    pub is_carrier: bool,
    /// MIDI, when the stream holds a `mid` chunk instead of audio.
    #[serde(skip)]
    pub midi: Option<Vec<u8>>,
    /// The stream's own coded audio, de-interleaved and decrypted.
    #[serde(skip)]
    pub data: Vec<u8>,
}

impl Stream {
    /// Whether there is audio here to play.
    pub fn is_playable(&self) -> bool {
        !self.is_carrier
            && self.midi.is_none()
            && !self.data.is_empty()
            && self.format.is_some_and(|f| f.sample_rate > 0)
    }

    /// Decode to 16-bit PCM samples.
    pub fn samples(&self) -> Vec<i16> {
        let Some(format) = self.format else {
            return Vec::new();
        };
        match format.codec {
            Codec::Adpcm => adpcm::decode(&self.data, format.samples as usize),
            // Anything unrecognised is read as PCM, which is what the
            // reference's fall-through does. It is audible garbage rather than
            // silence, which at least says the codec was wrong.
            Codec::Pcm | Codec::Unknown(_) => {
                let mut samples = wav::samples_from_le_bytes(&self.data);
                samples.truncate(format.samples as usize);
                samples
            }
        }
    }

    /// The stream as a `.wav` file.
    pub fn to_wav(&self) -> Vec<u8> {
        let rate = self.format.map(|f| f.sample_rate).unwrap_or(0);
        wav::mono_pcm16(&self.samples(), rate as u32)
    }
}

/// A parsed `.awc`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AwcFile {
    pub version: u16,
    pub flags: u16,
    /// True when the magic did not match and the whole file had to be
    /// decrypted before anything could be read.
    pub whole_file_encrypted: bool,
    pub multi_channel: bool,
    pub streams: Vec<Stream>,
}

impl AwcFile {
    fn chunk_indices_flag(flags: u16) -> bool {
        flags & 1 == 1
    }
    fn single_channel_encrypt_flag(flags: u16) -> bool {
        flags & 2 == 2
    }
    fn multi_channel_flag(flags: u16) -> bool {
        flags & 4 == 4
    }
    fn multi_channel_encrypt_flag(flags: u16) -> bool {
        flags & 8 == 8
    }

    /// Parse an extracted `.awc` payload.
    ///
    /// `awc_key` is `GtaKeys::awc_key`, the four words `UseMagicData` unmasks
    /// last. Taking the key rather than the whole key set keeps this readable
    /// without a game install, and states what it actually depends on.
    pub fn read(data: &[u8], awc_key: &[u32; 4]) -> Result<Self> {
        if data.len() < 16 {
            return Err(CwError::Invalid {
                what: "AWC file",
                detail: format!("{} bytes is too short for a header", data.len()),
            });
        }

        // A file whose magic does not match is encrypted whole. There is no
        // flag for this: failing to read `ADAT` *is* the signal.
        let mut owned;
        let mut whole_file_encrypted = false;
        let mut view = data;
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if magic != MAGIC_LE && magic != MAGIC_BE {
            if !data.len().is_multiple_of(4) {
                return Err(CwError::Invalid {
                    what: "AWC file",
                    detail: format!(
                        "magic {magic:#010x} is not ADAT and the length is not a multiple of 4, \
                         so it cannot be encrypted either"
                    ),
                });
            }
            owned = data.to_vec();
            decrypt_rsxxtea(&mut owned, awc_key);
            whole_file_encrypted = true;
            view = &owned;
        }

        let magic = u32::from_le_bytes([view[0], view[1], view[2], view[3]]);
        match magic {
            MAGIC_LE => {}
            MAGIC_BE => {
                // The reference switches its whole reader to big-endian here.
                // `DataReader` in this port is little-endian by design (see
                // `utils/data.rs`), so a console file is refused rather than
                // misread.
                return Err(CwError::Invalid {
                    what: "AWC file",
                    detail: "this is a big-endian console file; only PC files are supported".into(),
                });
            }
            other => {
                return Err(CwError::Invalid {
                    what: "AWC file",
                    detail: format!("magic {other:#010x} is not ADAT, even after decryption"),
                })
            }
        }

        let mut r = DataReader::new(view);
        let _magic = r.read_u32()?;
        let version = r.read_u16()?;
        let flags = r.read_u16()?;
        let stream_count = r.read_u32()? as usize;
        let _data_offset = r.read_u32()?;

        if Self::chunk_indices_flag(flags) {
            // Index of each stream's first chunk. Derivable from the counts
            // below, and the reference never reads it back, so it is skipped.
            for _ in 0..stream_count {
                r.read_u16()?;
            }
        }

        // Stream ids and chunk counts, then every chunk table back to back.
        let mut ids = Vec::with_capacity(stream_count);
        let mut chunk_counts = Vec::with_capacity(stream_count);
        for _ in 0..stream_count {
            let raw = r.read_u32()?;
            chunk_counts.push((raw >> 29) as usize);
            ids.push(raw & 0x1FFF_FFFF);
        }

        let mut chunk_table = Vec::with_capacity(stream_count);
        for count in &chunk_counts {
            let mut chunks = Vec::with_capacity(*count);
            for _ in 0..*count {
                let raw = r.read_u64()?;
                chunks.push(ChunkInfo {
                    kind: ChunkType::from_u8((raw >> 56) as u8),
                    size: ((raw >> 28) & 0x0FFF_FFFF) as usize,
                    offset: (raw & 0x0FFF_FFFF) as usize,
                });
            }
            chunk_table.push(chunks);
        }

        let multi_channel = Self::multi_channel_flag(flags);
        let mut streams = Vec::with_capacity(stream_count);
        let mut carrier: Option<Carrier> = None;

        for (i, chunks) in chunk_table.iter().enumerate() {
            let mut format = None;
            let mut stream_format = None;
            let mut midi = None;
            let mut data_chunk: Option<&[u8]> = None;

            for chunk in chunks {
                let Some(body) = view.get(chunk.offset..chunk.offset + chunk.size) else {
                    // A chunk pointing outside the file is skipped, not fatal:
                    // one bad chunk must not lose the other streams.
                    continue;
                };
                match chunk.kind {
                    ChunkType::Format => format = Some(read_format(body)?),
                    ChunkType::StreamFormat => stream_format = Some(read_stream_format(body)?),
                    ChunkType::Data => data_chunk = Some(body),
                    ChunkType::Midi => midi = Some(body.to_vec()),
                    // `seektable` is one sample offset per block. The reference
                    // reads it only to annotate each block for display; playing
                    // from the start does not need it, and neither does
                    // de-interleaving. Listed in `chunks`, not parsed.
                    _ => {}
                }
            }

            let mut data = Vec::new();
            let mut is_carrier = false;

            if let Some(body) = data_chunk {
                if multi_channel {
                    // This stream carries everyone's audio. Split it into
                    // blocks, decrypt each, and de-interleave the channels.
                    if let Some(stream_format) = &stream_format {
                        let channels = split_blocks(
                            body,
                            stream_format,
                            Self::multi_channel_encrypt_flag(flags) && !whole_file_encrypted,
                            awc_key,
                        );
                        carrier = Some(Carrier {
                            formats: stream_format.channels.clone(),
                            channels,
                        });
                        is_carrier = true;
                    }
                } else {
                    data = body.to_vec();
                    if Self::single_channel_encrypt_flag(flags) && !whole_file_encrypted {
                        decrypt_rsxxtea(&mut data, awc_key);
                    }
                }
            }

            streams.push(Stream {
                id: ids[i],
                chunks: chunks.clone(),
                format,
                is_carrier,
                midi,
                data,
            });
        }

        // Hand each non-carrier stream its channel out of the carrier, matched
        // by id the way `AssignMultiChannelSources` does.
        if let Some(carrier) = carrier {
            for stream in &mut streams {
                if stream.is_carrier {
                    continue;
                }
                let Some(index) = carrier.formats.iter().position(|(id, _)| *id == stream.id)
                else {
                    continue;
                };
                stream.format = Some(carrier.formats[index].1);
                if let Some(data) = carrier.channels.get(index) {
                    stream.data = data.clone();
                }
            }
        }

        Ok(Self {
            version,
            flags,
            whole_file_encrypted,
            multi_channel,
            streams,
        })
    }
}

/// The de-interleaved audio of a multi-channel file, and each channel's format.
struct Carrier {
    formats: Vec<(u32, Format)>,
    channels: Vec<Vec<u8>>,
}

/// A `streamformat` chunk: how the carrier's blocks are laid out, and one
/// format per channel.
struct StreamFormat {
    block_count: usize,
    block_size: usize,
    channels: Vec<(u32, Format)>,
}

fn read_format(body: &[u8]) -> Result<Format> {
    let mut r = DataReader::new(body);
    let samples = r.read_u32()?;
    let _loop_point = r.read_i32()?;
    let sample_rate = r.read_u16()?;
    let headroom = r.read_i16()?;
    let _loop_begin = r.read_u16()?;
    let _loop_end = r.read_u16()?;
    let _play_end = r.read_u16()?;
    let _play_begin = r.read_u8()?;
    let codec = Codec::from_u8(r.read_u8()?);
    // A 24-byte chunk carries a trailing peak word. The reference reads it and
    // does nothing with it either.
    Ok(Format {
        samples,
        sample_rate,
        codec,
        headroom,
    })
}

fn read_stream_format(body: &[u8]) -> Result<StreamFormat> {
    let mut r = DataReader::new(body);
    let block_count = r.read_u32()? as usize;
    let block_size = r.read_u32()? as usize;
    let channel_count = r.read_u32()? as usize;

    let mut channels = Vec::with_capacity(channel_count);
    for _ in 0..channel_count {
        let id = r.read_u32()?;
        let samples = r.read_u32()?;
        let headroom = r.read_i16()?;
        let sample_rate = r.read_u16()?;
        let codec = Codec::from_u8(r.read_u8()?);
        let _unused1 = r.read_u8()?;
        let _unused2 = r.read_u16()?;
        channels.push((
            id,
            Format {
                samples,
                sample_rate,
                codec,
                headroom,
            },
        ));
    }

    Ok(StreamFormat {
        block_count,
        block_size,
        channels,
    })
}

/// De-interleave a carrier's data chunk into one buffer per channel.
///
/// Each block opens with a 24-byte header per channel, then each channel's
/// per-sub-block sample offsets, then padding to a 2048-byte boundary, then the
/// channels' audio back to back at `block_count * 2048` bytes each.
///
/// A block that does not parse is skipped rather than failing the file: the
/// last block of a stream is routinely short.
fn split_blocks(
    data: &[u8],
    format: &StreamFormat,
    encrypted: bool,
    key: &[u32; 4],
) -> Vec<Vec<u8>> {
    let channel_count = format.channels.len();
    let mut out = vec![Vec::new(); channel_count];
    if format.block_size == 0 || channel_count == 0 {
        return out;
    }

    for b in 0..format.block_count {
        let start = b * format.block_size;
        if start >= data.len() {
            break;
        }
        let end = (start + format.block_size).min(data.len());

        let mut block = data[start..end].to_vec();
        if encrypted {
            decrypt_rsxxtea(&mut block, key);
        }

        let mut r = DataReader::new(&block);
        let mut headers = Vec::with_capacity(channel_count);
        let mut ok = true;
        for _ in 0..channel_count {
            let Ok(_start_block) = r.read_i32() else {
                ok = false;
                break;
            };
            let Ok(block_count) = r.read_i32() else {
                ok = false;
                break;
            };
            let (Ok(_unused1), Ok(_samples), Ok(_unused2), Ok(_unused3)) =
                (r.read_i32(), r.read_i32(), r.read_i32(), r.read_i32())
            else {
                ok = false;
                break;
            };
            headers.push(block_count.max(0) as usize);
        }
        if !ok {
            continue;
        }

        // Per-channel sample offsets, one i32 per sub-block. Read past: they
        // are seek points, and playing from the start does not need them.
        for count in &headers {
            if r.read_bytes(count * 4).is_err() {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }

        let pad = (BLOCK_ALIGN - (r.position() % BLOCK_ALIGN)) % BLOCK_ALIGN;
        if r.read_bytes(pad).is_err() {
            continue;
        }

        for (channel, count) in headers.iter().enumerate() {
            match r.read_bytes(count * 2048) {
                Ok(bytes) => out[channel].extend_from_slice(bytes),
                // A short final block still yields whatever it holds for the
                // earlier channels; later ones simply end here.
                Err(_) => break,
            }
        }
    }

    out
}

/// Rockstar's modified XXTEA, decrypting in place.
///
/// Port of `AwcFile.Decrypt_RSXXTEA`. Two things differ from the reference
/// XXTEA on Wikipedia, and both are load-bearing:
///
/// - The round constant is folded into the key lookup as `^ 0x7B3A207F`.
/// - The decrypt loop seeds `b` from `blocks[0]` rather than from the last
///   block, which is not what a symmetric XXTEA decrypt does. The reference
///   calls this out as undefined behaviour it has not cleaned up; it is
///   reproduced verbatim, because the encryptor in the game agrees with it.
///
/// A trailing partial word is left alone — XXTEA has nothing to say about it.
pub fn decrypt_rsxxtea(data: &mut [u8], key: &[u32; 4]) {
    let n = data.len() / 4;
    if n == 0 {
        return;
    }

    let mut blocks: Vec<u32> = data[..n * 4]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let mut b = blocks[0];
    let mut i = 0x9E37_79B9u32.wrapping_mul(6 + 52 / n as u32);

    loop {
        for index in (0..n).rev() {
            let a = blocks[if index > 0 { index } else { n } - 1];
            let k = key[(index & 3) ^ ((i >> 2) as usize & 3)];
            let mx = (((a >> 5) ^ (b << 2)).wrapping_add((b >> 3) ^ (a << 4)))
                ^ ((i ^ b).wrapping_add(k ^ a ^ 0x7B3A_207F));
            blocks[index] = blocks[index].wrapping_sub(mx);
            b = blocks[index];
        }
        i = i.wrapping_sub(0x9E37_79B9);
        if i == 0 {
            break;
        }
    }

    for (word, out) in blocks.iter().zip(data.chunks_exact_mut(4)) {
        out.copy_from_slice(&word.to_le_bytes());
    }
}

/// The inverse, kept beside the decryptor because it is the only statement of
/// what the decryptor is undoing. Port of `AwcFile.Encrypt_RSXXTEA`.
pub fn encrypt_rsxxtea(data: &mut [u8], key: &[u32; 4]) {
    let n = data.len() / 4;
    if n == 0 {
        return;
    }

    let mut blocks: Vec<u32> = data[..n * 4]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let mut rounds = 6 + 52 / n;
    let mut sum = 0u32;
    let mut z = blocks[n - 1];

    while rounds > 0 {
        sum = sum.wrapping_add(0x9E37_79B9);
        let e = ((sum >> 2) & 3) as usize;
        for p in 0..n {
            let y = blocks[(p + 1) % n];
            let k = key[(p & 3) ^ e];
            let mx = (((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4)))
                ^ ((sum ^ y).wrapping_add(k ^ z ^ 0x7B3A_207F));
            blocks[p] = blocks[p].wrapping_add(mx);
            z = blocks[p];
        }
        rounds -= 1;
    }

    for (word, out) in blocks.iter().zip(data.chunks_exact_mut(4)) {
        out.copy_from_slice(&word.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u32; 4] = [0x1234_5678, 0x9ABC_DEF0, 0x0F1E_2D3C, 0xDEAD_BEEF];

    #[test]
    fn maps_the_codec_values_the_reference_names() {
        assert_eq!(Codec::from_u8(0), Codec::Pcm);
        assert_eq!(Codec::from_u8(4), Codec::Adpcm);
        assert_eq!(Codec::from_u8(9), Codec::Unknown(9));
    }

    /// The chunk type byte is the last byte of the Jenkins hash of the chunk's
    /// name, so the values look arbitrary and a typo would not be obvious.
    #[test]
    fn maps_the_chunk_type_bytes() {
        assert_eq!(ChunkType::from_u8(0x55), ChunkType::Data);
        assert_eq!(ChunkType::from_u8(0xFA), ChunkType::Format);
        assert_eq!(ChunkType::from_u8(0x48), ChunkType::StreamFormat);
        assert_eq!(ChunkType::from_u8(0xA3), ChunkType::SeekTable);
        assert_eq!(ChunkType::from_u8(0x68), ChunkType::Midi);
        assert_eq!(ChunkType::from_u8(0x00), ChunkType::Unknown(0));
    }

    /// The decrypt reads as if it were asymmetric — it seeds `b` from
    /// `blocks[0]` where the textbook version seeds from the last block — and
    /// the reference calls its own code undefined behaviour it has not cleaned
    /// up. It is not asymmetric: at `p = n-1` the textbook `y` *is* `blocks[0]`,
    /// and every later iteration inherits the right value. This pins that, so a
    /// future tidy-up cannot quietly change which one is true.
    ///
    /// Starts at two words: XXTEA is undefined for a single block, and there
    /// the two directions genuinely disagree. No real AWC payload is four bytes
    /// long, and the reference has the same hole.
    #[test]
    fn encrypt_and_decrypt_are_inverses_despite_how_the_decrypt_reads() {
        for words in [2usize, 3, 13, 52, 53, 200] {
            let original: Vec<u8> = (0..words * 4).map(|i| (i * 31 + 7) as u8).collect();

            let mut round_tripped = original.clone();
            encrypt_rsxxtea(&mut round_tripped, &KEY);
            assert_ne!(round_tripped, original, "{words} words: nothing happened");

            decrypt_rsxxtea(&mut round_tripped, &KEY);
            assert_eq!(round_tripped, original, "{words} words did not round-trip");
        }
    }

    /// Decryption must depend on every key word and on position, or a wrong key
    /// would produce something that still looked structured.
    #[test]
    fn decryption_depends_on_the_key() {
        let mut a = vec![0u8; 64];
        let mut b = vec![0u8; 64];
        decrypt_rsxxtea(&mut a, &KEY);
        decrypt_rsxxtea(&mut b, &[KEY[0] ^ 1, KEY[1], KEY[2], KEY[3]]);
        assert_ne!(a, b);
        assert_ne!(a, vec![0u8; 64]);
    }

    #[test]
    fn leaves_a_buffer_too_short_to_have_a_word_alone() {
        let mut data = vec![1u8, 2, 3];
        decrypt_rsxxtea(&mut data, &KEY);
        assert_eq!(data, vec![1, 2, 3]);
    }

    /// A trailing partial word is outside XXTEA's block structure and must come
    /// through untouched.
    #[test]
    fn leaves_a_trailing_partial_word_alone() {
        let mut data = vec![0u8; 4 * 4 + 3];
        data[16..].copy_from_slice(&[0xAA, 0xBB, 0xCC]);
        decrypt_rsxxtea(&mut data, &KEY);
        assert_eq!(&data[16..], &[0xAA, 0xBB, 0xCC]);
    }

    /// Fewer than 52 words means extra rounds; more means exactly six. Both
    /// paths have to terminate.
    #[test]
    fn terminates_for_block_counts_on_both_sides_of_the_round_formula() {
        for words in [1usize, 2, 13, 52, 53, 200] {
            let mut data = vec![7u8; words * 4];
            decrypt_rsxxtea(&mut data, &KEY);
            assert_ne!(data, vec![7u8; words * 4], "{words} words did nothing");
        }
    }

    /// Build a minimal single-stream PCM file: header, one stream info, one
    /// chunk info each for `format` and `data`.
    fn synthetic_awc(samples: &[i16]) -> Vec<u8> {
        let stream_count = 1u32;
        // 16-byte header + 4 bytes of stream info + 2 chunk infos.
        let data_offset = 16 + 4 + 16;
        let format_offset = data_offset;
        let audio_offset = format_offset + 20;

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_LE.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // version
        out.extend_from_slice(&0xFF00u16.to_le_bytes()); // flags: none set
        out.extend_from_slice(&stream_count.to_le_bytes());
        out.extend_from_slice(&(data_offset as u32).to_le_bytes());

        // Stream info: 2 chunks, id 0x0BADF00D masked to 29 bits.
        let id = 0x0BAD_F00Du32 & 0x1FFF_FFFF;
        out.extend_from_slice(&(id + (2u32 << 29)).to_le_bytes());

        let chunk = |kind: u64, size: u64, offset: u64| offset | (size << 28) | (kind << 56);
        out.extend_from_slice(&chunk(0xFA, 20, format_offset as u64).to_le_bytes());
        out.extend_from_slice(
            &chunk(0x55, (samples.len() * 2) as u64, audio_offset as u64).to_le_bytes(),
        );

        // format chunk
        out.extend_from_slice(&(samples.len() as u32).to_le_bytes()); // samples
        out.extend_from_slice(&0i32.to_le_bytes()); // loop point
        out.extend_from_slice(&16_000u16.to_le_bytes()); // sample rate
        out.extend_from_slice(&(-300i16).to_le_bytes()); // headroom
        out.extend_from_slice(&0u16.to_le_bytes()); // loop begin
        out.extend_from_slice(&0u16.to_le_bytes()); // loop end
        out.extend_from_slice(&0u16.to_le_bytes()); // play end
        out.push(0); // play begin
        out.push(0); // codec: PCM

        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    #[test]
    fn reads_a_single_stream_pcm_file() {
        let samples: Vec<i16> = (0..64).map(|i| (i * 100) as i16).collect();
        let awc = AwcFile::read(&synthetic_awc(&samples), &KEY).expect("parse");

        assert_eq!(awc.version, 1);
        assert!(!awc.whole_file_encrypted);
        assert!(!awc.multi_channel);
        assert_eq!(awc.streams.len(), 1);

        let stream = &awc.streams[0];
        assert_eq!(stream.id, 0x0BAD_F00D & 0x1FFF_FFFF);
        assert_eq!(stream.chunks.len(), 2);
        assert_eq!(stream.chunks[0].kind, ChunkType::Format);
        assert_eq!(stream.chunks[1].kind, ChunkType::Data);

        let format = stream.format.expect("a format chunk");
        assert_eq!(format.codec, Codec::Pcm);
        assert_eq!(format.sample_rate, 16_000);
        assert_eq!(format.samples, 64);
        assert_eq!(format.headroom, -300);
        assert!((format.seconds() - 0.004).abs() < 1e-6);

        assert!(stream.is_playable());
        assert_eq!(stream.samples(), samples);

        let wav = stream.to_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 44 + samples.len() * 2);
    }

    /// The whole-file case has no flag: the magic simply fails to match, and
    /// the reader has to try decrypting before giving up.
    #[test]
    fn detects_and_decrypts_a_whole_file_encrypted_container() {
        let samples: Vec<i16> = (0..64).map(|i| (i * 7) as i16).collect();
        let plain = synthetic_awc(&samples);

        let mut encrypted = plain.clone();
        while !encrypted.len().is_multiple_of(4) {
            encrypted.push(0);
        }
        encrypt_rsxxtea(&mut encrypted, &KEY);
        assert_ne!(&encrypted[..4], &MAGIC_LE.to_le_bytes(), "still readable");

        let awc = AwcFile::read(&encrypted, &KEY).expect("parse");
        assert!(awc.whole_file_encrypted);
        assert_eq!(awc.streams[0].samples(), samples);
    }

    /// The wrong key produces a payload that is not ADAT either, and that has
    /// to be an error rather than a plausible-looking parse.
    #[test]
    fn reports_a_file_it_cannot_decrypt() {
        let mut encrypted = synthetic_awc(&[1, 2, 3, 4]);
        while !encrypted.len().is_multiple_of(4) {
            encrypted.push(0);
        }
        encrypt_rsxxtea(&mut encrypted, &KEY);

        let wrong = [KEY[0] ^ 1, KEY[1], KEY[2], KEY[3]];
        let error = AwcFile::read(&encrypted, &wrong).expect_err("the key is wrong");
        match error {
            CwError::Invalid { detail, .. } => assert!(
                detail.contains("even after decryption"),
                "the reader should say it tried: {detail}"
            ),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_a_big_endian_console_file() {
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(&MAGIC_BE.to_le_bytes());
        let error = AwcFile::read(&data, &KEY).expect_err("console files are out of scope");
        assert!(format!("{error}").contains("big-endian"));
    }

    #[test]
    fn rejects_a_payload_too_short_for_a_header() {
        assert!(AwcFile::read(&[0u8; 8], &KEY).is_err());
    }

    /// A chunk whose offset runs past the end of the file is skipped, not
    /// fatal. Mod archives ship these.
    #[test]
    fn skips_a_chunk_that_points_outside_the_file() {
        let mut data = synthetic_awc(&[1, 2, 3, 4]);
        // Rewrite the data chunk's offset to something well past the end.
        let chunk_at = 16 + 4 + 8;
        let raw = u64::from_le_bytes(data[chunk_at..chunk_at + 8].try_into().unwrap());
        let rewritten = (raw & !0x0FFF_FFFF) | 0x00FF_FFFF;
        data[chunk_at..chunk_at + 8].copy_from_slice(&rewritten.to_le_bytes());

        let awc = AwcFile::read(&data, &KEY).expect("the file should still open");
        assert!(!awc.streams[0].is_playable(), "there is no audio to play");
        assert!(
            awc.streams[0].format.is_some(),
            "the format chunk still read"
        );
    }
}
