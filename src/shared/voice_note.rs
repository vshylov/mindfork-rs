//! Voice notes: OGG/Opus, mono — what Telegram sends from a phone and shows as a
//! voice message when a bot sends one (ADR 0013, stage C, rule 1).
//!
//! Both ways run in this process, in Rust: [`encode`] writes a reply's samples as
//! a note, [`decode`] turns a note into samples for a recogniser that takes
//! nothing but WAV ([`wav`]). The codec is `opus-rs`, a port of libopus 1.6 with
//! no C and no dependencies; the container is the `ogg` crate. Measured before it
//! was chosen (docs/research/companion-platform.md §3.8): a note written so is
//! read by ffmpeg without a complaint and heard word for word by a recogniser,
//! and a phone's note decoded so is heard by llama.cpp.

use std::io::Cursor;

use ogg::{PacketReader, PacketWriteEndInfo, PacketWriter};
use opus_rs::{Application, OpusDecoder, OpusEncoder};

/// The rates Opus encodes from. Anything else is resampled up to the next one.
const OPUS_RATES: [u32; 5] = [8_000, 12_000, 16_000, 24_000, 48_000];

/// The bitrate a note is written at: a phone's own voice notes are about this.
const BITRATE: i32 = 32_000;

/// The encoder's lookahead at 48 kHz — 2.5 ms plus libopus's 4 ms delay
/// compensation — which a player skips (`OpusHead`'s pre-skip).
const PRE_SKIP: u16 = 312;

/// The rate [`decode`] hands samples back at: what speech recognisers want.
pub const DECODE_RATE: u32 = 16_000;

/// The longest Opus packet, in samples per channel at 48 kHz (120 ms).
const MAX_PACKET_48K: usize = 5_760;

/// A note's stream serial: any fixed number does for a single stream.
const SERIAL: u32 = 0x6d66_766e;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VoiceNoteError {
    /// Not an OGG stream, or not Opus inside one.
    #[error("not an OGG/Opus voice note")]
    NotOpus,
    /// The codec refused a frame, in its own words.
    #[error("the Opus codec failed: {0}")]
    Codec(String),
    /// A note with no sound in it.
    #[error("the voice note is empty")]
    Empty,
}

/// `samples` (mono, signed 16-bit) at `rate` written as an OGG/Opus voice note.
pub fn encode(samples: &[i16], rate: u32) -> Result<Vec<u8>, VoiceNoteError> {
    if samples.is_empty() || rate == 0 {
        return Err(VoiceNoteError::Empty);
    }
    let target = OPUS_RATES
        .iter()
        .copied()
        .find(|r| *r >= rate)
        .unwrap_or(48_000);
    let resampled;
    let samples = if target == rate {
        samples
    } else {
        resampled = resample(samples, rate, target);
        &resampled
    };
    let mut enc = OpusEncoder::new(target as i32, 1, Application::Voip)
        .map_err(|e| VoiceNoteError::Codec(e.to_string()))?;
    enc.bitrate_bps = BITRATE;
    let frame = (target / 50) as usize; // 20 ms
    let per_frame_48k = 960u64;
    let total_48k = samples.len() as u64 * 48_000 / u64::from(target);

    let mut out = Vec::new();
    let mut w = PacketWriter::new(Cursor::new(&mut out));
    let write = |w: &mut PacketWriter<'_, Cursor<&mut Vec<u8>>>,
                 packet: Vec<u8>,
                 end: PacketWriteEndInfo,
                 granule: u64| {
        w.write_packet(packet, SERIAL, end, granule)
            .map_err(|e| VoiceNoteError::Codec(e.to_string()))
    };
    write(&mut w, opus_head(target), PacketWriteEndInfo::EndPage, 0)?;
    write(&mut w, opus_tags(), PacketWriteEndInfo::EndPage, 0)?;
    let frames = samples.len().div_ceil(frame);
    let mut buf = vec![0u8; 4_000];
    let mut padded = vec![0i16; frame];
    for (i, chunk) in samples.chunks(frame).enumerate() {
        padded[..chunk.len()].copy_from_slice(chunk);
        padded[chunk.len()..].fill(0);
        let n = enc
            .encode_i16(&padded, frame, &mut buf)
            .map_err(|e| VoiceNoteError::Codec(e.to_string()))?;
        let last = i + 1 == frames;
        // The last page's granule marks where the sound ends, so the padding of
        // the last frame is not played.
        let granule = if last {
            total_48k + u64::from(PRE_SKIP)
        } else {
            (i as u64 + 1) * per_frame_48k + u64::from(PRE_SKIP)
        };
        let end = if last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        write(&mut w, buf[..n].to_vec(), end, granule)?;
    }
    drop(w);
    Ok(out)
}

/// An OGG/Opus note's sound, mono at [`DECODE_RATE`], the encoder's lookahead
/// skipped.
pub fn decode(note: &[u8]) -> Result<Vec<i16>, VoiceNoteError> {
    let mut r = PacketReader::new(Cursor::new(note));
    let head = r
        .read_packet()
        .map_err(|_| VoiceNoteError::NotOpus)?
        .ok_or(VoiceNoteError::NotOpus)?;
    if head.data.len() < 19 || &head.data[..8] != b"OpusHead" {
        return Err(VoiceNoteError::NotOpus);
    }
    let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]);
    let mut dec = OpusDecoder::new(DECODE_RATE as i32, 1)
        .map_err(|e| VoiceNoteError::Codec(e.to_string()))?;
    let max = MAX_PACKET_48K * DECODE_RATE as usize / 48_000;
    let mut pcm = vec![0f32; max];
    let mut out: Vec<i16> = Vec::new();
    let mut tags_seen = false;
    while let Some(p) = r.read_packet().map_err(|_| VoiceNoteError::NotOpus)? {
        if !tags_seen {
            tags_seen = true;
            if p.data.starts_with(b"OpusTags") {
                continue;
            }
        }
        let got = dec
            .decode(&p.data, max, &mut pcm)
            .map_err(|e| VoiceNoteError::Codec(e.to_string()))?;
        out.extend(
            pcm[..got]
                .iter()
                .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16),
        );
    }
    let skip = usize::from(pre_skip) * DECODE_RATE as usize / 48_000;
    if out.len() <= skip {
        return Err(VoiceNoteError::Empty);
    }
    out.drain(..skip);
    Ok(out)
}

/// `samples` (mono, signed 16-bit) at `rate` as a WAV file.
pub fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// `samples` from `from` Hz to `to` Hz, by linear interpolation — enough for
/// speech, where a rate is only ever raised to the next one Opus takes, or
/// pieces of one reply brought to one rate.
pub fn resample(samples: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || samples.is_empty() || from == 0 || to == 0 {
        return samples.to_vec();
    }
    let len = (samples.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    let step = f64::from(from) / f64::from(to);
    (0..len)
        .map(|i| {
            let pos = i as f64 * step;
            let at = pos as usize;
            let frac = pos - at as f64;
            let a = f64::from(samples[at.min(samples.len() - 1)]);
            let b = f64::from(samples[(at + 1).min(samples.len() - 1)]);
            (a + (b - a) * frac).round() as i16
        })
        .collect()
}

/// `OpusHead` (RFC 7845 §5.1): version 1, mono, the pre-skip, the input rate,
/// no gain, mapping family 0.
fn opus_head(input_rate: u32) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(1);
    head.extend_from_slice(&PRE_SKIP.to_le_bytes());
    head.extend_from_slice(&input_rate.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

/// `OpusTags` (RFC 7845 §5.2): the vendor, no comments.
fn opus_tags() -> Vec<u8> {
    let vendor = b"mindfork";
    let mut tags = Vec::with_capacity(16 + vendor.len());
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second of a 440 Hz tone at `rate`, at half scale.
    fn tone(rate: u32, seconds: f64) -> Vec<i16> {
        let n = (f64::from(rate) * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                ((2.0 * std::f64::consts::PI * 440.0 * t).sin() * 16_000.0) as i16
            })
            .collect()
    }

    fn rms(samples: &[i16]) -> f64 {
        let sum: f64 = samples.iter().map(|s| f64::from(*s).powi(2)).sum();
        (sum / samples.len().max(1) as f64).sqrt()
    }

    #[test]
    fn a_note_is_an_ogg_opus_stream_a_player_reads() {
        let note = encode(&tone(24_000, 1.0), 24_000).unwrap();
        assert_eq!(&note[..4], b"OggS", "an OGG page first");
        assert!(
            note.windows(8).any(|w| w == b"OpusHead"),
            "an Opus stream inside"
        );
        // 32 kbps for a second is about 4 KB; a raw second at 24 kHz is 48 KB.
        assert!(note.len() < 12_000, "{} bytes", note.len());
    }

    #[test]
    fn what_is_written_reads_back_as_the_same_length_of_sound() {
        let input = tone(24_000, 1.5);
        let back = decode(&encode(&input, 24_000).unwrap()).unwrap();
        let want = (1.5 * f64::from(DECODE_RATE)) as usize;
        let diff = back.len().abs_diff(want);
        // Whole 20 ms frames are decoded: the last one's padding comes back too.
        assert!(diff <= 320, "{} samples back, {want} wanted", back.len());
        let (r_in, r_out) = (rms(&input), rms(&back[1_000..back.len() - 1_000]));
        assert!(
            (r_out / r_in - 1.0).abs() < 0.25,
            "the tone kept its loudness: {r_in:.0} in, {r_out:.0} out"
        );
    }

    #[test]
    fn a_rate_opus_does_not_take_is_raised_to_one_it_does() {
        // 22.05 kHz, a common container rate, is written from 24 kHz.
        let note = encode(&tone(22_050, 1.0), 22_050).unwrap();
        let head = note
            .windows(19)
            .find(|w| w.starts_with(b"OpusHead"))
            .unwrap();
        assert_eq!(u32::from_le_bytes(head[12..16].try_into().unwrap()), 24_000);
        let back = decode(&note).unwrap();
        assert!(back.len().abs_diff(16_000) <= 320, "{}", back.len());
    }

    #[test]
    fn nothing_to_say_and_a_stranger_file_are_refused() {
        assert_eq!(encode(&[], 24_000), Err(VoiceNoteError::Empty));
        assert_eq!(decode(b"not a note"), Err(VoiceNoteError::NotOpus));
        // An OGG stream of something else: its first packet is no `OpusHead`.
        let mut other = Vec::new();
        let mut w = PacketWriter::new(Cursor::new(&mut other));
        w.write_packet(b"\x01vorbis".to_vec(), 1, PacketWriteEndInfo::EndStream, 0)
            .unwrap();
        drop(w);
        assert_eq!(decode(&other), Err(VoiceNoteError::NotOpus));
    }

    #[test]
    fn a_wav_header_names_its_rate_and_length() {
        let w = wav(&[1, -1, 2], 16_000);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(w.len(), 44 + 6);
    }

    #[test]
    fn resampling_keeps_the_length_in_time() {
        let s = tone(22_050, 1.0);
        assert_eq!(resample(&s, 22_050, 24_000).len(), 24_000);
        assert_eq!(resample(&s, 22_050, 22_050), s);
        assert_eq!(resample(&[], 8_000, 16_000), Vec::<i16>::new());
    }
}
