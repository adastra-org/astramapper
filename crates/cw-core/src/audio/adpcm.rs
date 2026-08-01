//! IMA ADPCM, as GTA V blocks it.
//!
//! Port of `ADPCMCodec.DecodeADPCM` at the bottom of
//! `CodeWalker.Core/GameFiles/FileTypes/AwcFile.cs`.
//!
//! Standard IMA nibbles, but the framing is Rockstar's: 2048-byte blocks, each
//! opening with a 4-byte header of `{step index, pad, predictor:i16}` and
//! carrying 2044 bytes of nibble pairs. The predictor and step index reset at
//! every block, which is what makes seeking possible and what makes decoding a
//! block in isolation correct.

/// How the step index moves after each nibble. The table is duplicated across
/// the sign bit, so a nibble indexes it directly.
const INDEX_TABLE: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// Bytes of nibble data after each block's 4-byte header.
const BLOCK_PAYLOAD: usize = 2044;

/// Decode to signed 16-bit PCM samples.
///
/// `sample_count` is what the stream's format chunk declares, and it is
/// authoritative: the result is exactly that long for any stream that decoded
/// at all. Two things make that not a no-op.
///
/// **Short data is padded with silence.** Blocks hold 4088 samples, so a stream
/// whose declared count is odd routinely has one sample more than its blocks can
/// produce — `01_0_gtaiv_legacy_support_small.awc` declares 12161 and carries
/// 12160. The reference lands in the same place by accident: `DecodeADPCM`
/// zeroes its output buffer up front and never writes that last sample.
///
/// **Long data is trimmed**, which the reference does *not* do. It allocates
/// `data.len() * 4` bytes and returns the whole buffer however little it wrote,
/// so the WAVs CodeWalker exports carry four bytes of silence per block header
/// past the end of the audio. Trimming is the deliberate divergence here; the
/// samples themselves are identical either way.
///
/// A stream that decoded nothing stays empty rather than becoming
/// `sample_count` zeros, so the caller can still tell there was no audio.
pub fn decode(data: &[u8], sample_count: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(sample_count.min(data.len() * 2));

    let mut predictor: i32 = 0;
    let mut step_index: i32 = 0;
    let mut bytes_in_block = 0usize;
    let mut at = 0usize;

    while at < data.len() && out.len() < sample_count {
        if bytes_in_block == 0 {
            if at + 4 > data.len() {
                break;
            }
            step_index = (data[at] as i32).clamp(0, 88);
            predictor = i16::from_le_bytes([data[at + 2], data[at + 3]]) as i32;
            bytes_in_block = BLOCK_PAYLOAD;
            at += 4;
            continue;
        }

        let byte = data[at];
        for nibble in [byte & 0x0F, (byte >> 4) & 0x0F] {
            let step = STEP_TABLE[step_index as usize];
            let mut diff = (((((nibble & 7) as i32) << 1) + 1) * step) >> 3;
            if nibble & 8 != 0 {
                diff = -diff;
            }
            predictor += diff;
            step_index = (step_index + INDEX_TABLE[nibble as usize]).clamp(0, 88);
            out.push(predictor.clamp(-32768, 32767) as i16);
        }
        bytes_in_block -= 1;
        at += 1;
    }

    if !out.is_empty() {
        out.resize(sample_count, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one block: the 4-byte header, then nibble pairs.
    fn block(step_index: u8, predictor: i16, nibbles: &[u8]) -> Vec<u8> {
        let mut out = vec![step_index, 0];
        out.extend_from_slice(&predictor.to_le_bytes());
        for pair in nibbles.chunks(2) {
            let low = pair[0] & 0x0F;
            let high = pair.get(1).copied().unwrap_or(0) & 0x0F;
            out.push(low | (high << 4));
        }
        out
    }

    /// Nibble 0 with step index 0 adds `((0<<1)+1) * 7 >> 3` = 0, so the
    /// predictor holds. This pins the header decode: a wrong predictor offset
    /// would show up immediately as a different first sample.
    #[test]
    fn starts_from_the_block_header_predictor() {
        let data = block(0, 1000, &[0, 0]);
        assert_eq!(decode(&data, 2), vec![1000, 1000]);
    }

    /// The low nibble of a byte is the first sample, the high nibble the
    /// second. Reversing them is the classic ADPCM porting error and would
    /// still produce plausible-sounding audio.
    #[test]
    fn reads_the_low_nibble_first() {
        // Nibble 4 with step 7: diff = ((4&7)<<1 + 1) * 7 >> 3 = 9*7>>3 = 7.
        // Nibble 0xC is the same magnitude with the sign bit set.
        let data = block(0, 0, &[4, 0xC]);
        let decoded = decode(&data, 2);
        assert!(decoded[0] > 0, "first sample should rise: {decoded:?}");
        assert!(
            decoded[1] < decoded[0],
            "second sample should fall: {decoded:?}"
        );
    }

    #[test]
    fn stops_at_the_declared_sample_count() {
        let data = block(0, 0, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(decode(&data, 3).len(), 3);
    }

    /// Blocks hold whole numbers of sample pairs, so an odd declared count is
    /// routinely one more than the data can produce. The stream is that long
    /// because the container says so.
    #[test]
    fn pads_a_stream_shorter_than_its_declared_length() {
        let data = block(0, 500, &[0, 0]);
        let decoded = decode(&data, 5);
        assert_eq!(decoded.len(), 5);
        assert_eq!(&decoded[..2], &[500, 500]);
        assert_eq!(&decoded[2..], &[0, 0, 0], "the tail is silence");
    }

    /// A step index past the end of the table is clamped rather than indexing
    /// out of bounds. The reference clamps on read for the same reason: the
    /// byte in the block header is not validated anywhere.
    #[test]
    fn clamps_a_step_index_past_the_end_of_the_table() {
        let data = block(200, 0, &[7, 7, 7, 7, 7, 7, 7, 7]);
        assert_eq!(decode(&data, 8).len(), 8);
    }

    /// A stream with nothing decodable stays empty rather than becoming a
    /// block of silence, so the caller can tell there was no audio.
    #[test]
    fn a_stream_that_decodes_nothing_stays_empty() {
        assert!(
            decode(&[0, 0], 100).is_empty(),
            "too short for a block header"
        );
        assert!(decode(&[], 100).is_empty());
        assert!(
            decode(&[0, 0, 0, 0], 100).is_empty(),
            "a header and no payload"
        );
    }

    /// The predictor saturates at the i16 range instead of wrapping, which is
    /// the difference between a clipped sample and a loud click.
    #[test]
    fn clamps_the_predictor_instead_of_wrapping() {
        // Step 32767 with nibble 7 adds 15*32767>>3 = 61437 to a predictor
        // already at 32000, which is well past i16. Wrapping would come back as
        // a large negative sample.
        let decoded = decode(&block(88, 32000, &[7, 7, 7, 7]), 4);
        assert_eq!(decoded[0], 32767, "should saturate, not wrap");
        assert!(
            decoded.iter().all(|s| *s > 0),
            "wrapped somewhere: {decoded:?}"
        );
    }
}
