//! Wrapping PCM samples in a RIFF/WAVE container.
//!
//! Port of `AwcStream.GetWavStream` in
//! `CodeWalker.Core/GameFiles/FileTypes/AwcFile.cs`, narrowed to the one case
//! that file actually writes: 16-bit PCM, mono. The reference's ADPCM-in-WAV
//! branch is commented out with "can't seem to get ADPCM wav files to work",
//! so ADPCM is decoded first and only PCM is ever wrapped.
//!
//! Mono because an AWC channel *is* one channel. A stereo sound is two streams
//! in the container, and pairing them needs the `_left`/`_right` name
//! convention the reference resolves through `JenkIndex` — which needs the name
//! tables from `MetaNames.cs`, not ported yet.

/// Bytes of RIFF header before the sample data.
pub const HEADER_LEN: usize = 44;

/// Wrap 16-bit mono samples as a `.wav` file.
pub fn mono_pcm16(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_len = samples.len() * 2;

    let mut out = Vec::with_capacity(HEADER_LEN + data_len);
    out.extend_from_slice(b"RIFF");
    // Everything after this field: 4 ("WAVE") + 8+16 (fmt) + 8+data.
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // WAVE_FORMAT_PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }

    out
}

/// Reinterpret raw little-endian PCM16 bytes as samples. A trailing odd byte is
/// dropped rather than half-read.
pub fn samples_from_le_bytes(data: &[u8]) -> Vec<i16> {
    data.chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_header_a_player_will_accept() {
        let wav = mono_pcm16(&[0, -1, 32767], 48_000);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), HEADER_LEN + 6);

        let riff_size = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        assert_eq!(riff_size as usize, wav.len() - 8);

        let data_size = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_size, 6);

        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 48_000);
        // Byte rate: 48000 * 1 channel * 2 bytes.
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 96_000);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
    }

    #[test]
    fn writes_samples_little_endian() {
        let wav = mono_pcm16(&[0x1234, -2], 8000);
        assert_eq!(&wav[HEADER_LEN..], &[0x34, 0x12, 0xFE, 0xFF]);
    }

    #[test]
    fn handles_an_empty_stream() {
        let wav = mono_pcm16(&[], 44_100);
        assert_eq!(wav.len(), HEADER_LEN);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
    }

    #[test]
    fn round_trips_raw_pcm_bytes() {
        let samples = samples_from_le_bytes(&[0x34, 0x12, 0xFE, 0xFF, 0x77]);
        assert_eq!(
            samples,
            vec![0x1234, -2],
            "the odd trailing byte is dropped"
        );
    }
}
