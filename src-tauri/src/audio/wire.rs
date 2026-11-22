// Trovul - UDP audio packet format (wire protocol v2).
//
//   offset size field
//   0      4    magic "TRV2"
//   4      4    session id   (u32 LE) — negotiated on the control link
//   8      4    sequence     (u32 LE, wrapping, random start per stream)
//   12     4    sample rate  (u32 LE)
//   16     2    frame count  (u16 LE)
//   18     1    channels     (1 or 2; the sender always downmixes to 2)
//   19     1    codec        (1 = PCM s16le)
//   20     …    payload      frame count × channels × 2 bytes
//
// A full packet is 5 ms of 48 kHz stereo (980 bytes), well under the Ethernet
// MTU, so packets never fragment at the IP layer. On Wi-Fi a single lost
// fragment used to cost the whole ~3.8 KB datagram.

pub const MAGIC: [u8; 4] = *b"TRV2";
pub const HEADER_LEN: usize = 20;
pub const CODEC_PCM_S16LE: u8 = 1;
pub const WIRE_CHANNELS: u8 = 2;
pub const FRAMES_PER_PACKET: usize = 240;
pub const MAX_PACKET_LEN: usize = HEADER_LEN + FRAMES_PER_PACKET * WIRE_CHANNELS as usize * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub session: u32,
    pub seq: u32,
    pub sample_rate: u32,
    pub frames: u16,
    pub channels: u8,
    pub codec: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    TooShort,
    BadMagic,
    Unsupported,
    BadLength,
}

/// Serialise one packet of stereo s16 frames into `out` (cleared first).
pub fn encode_packet(
    out: &mut Vec<u8>,
    session: u32,
    seq: u32,
    sample_rate: u32,
    frames: &[[i16; 2]],
) {
    debug_assert!(frames.len() <= u16::MAX as usize);
    out.clear();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&session.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    out.push(WIRE_CHANNELS);
    out.push(CODEC_PCM_S16LE);
    for [l, r] in frames {
        out.extend_from_slice(&l.to_le_bytes());
        out.extend_from_slice(&r.to_le_bytes());
    }
}

/// Validate a datagram and split it into header and payload.
pub fn decode_header(buf: &[u8]) -> Result<(PacketHeader, &[u8]), DecodeError> {
    if buf.len() < HEADER_LEN {
        return Err(DecodeError::TooShort);
    }
    if buf[0..4] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let u32_at = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
    let header = PacketHeader {
        session: u32_at(4),
        seq: u32_at(8),
        sample_rate: u32_at(12),
        frames: u16::from_le_bytes([buf[16], buf[17]]),
        channels: buf[18],
        codec: buf[19],
    };
    if header.codec != CODEC_PCM_S16LE
        || !(1..=2).contains(&header.channels)
        || !(8_000..=384_000).contains(&header.sample_rate)
    {
        return Err(DecodeError::Unsupported);
    }
    let payload = &buf[HEADER_LEN..];
    if payload.len() != header.frames as usize * header.channels as usize * 2 {
        return Err(DecodeError::BadLength);
    }
    Ok((header, payload))
}

/// Decode a PCM s16le payload into stereo f32 frames (mono goes to both sides).
pub fn for_each_frame(payload: &[u8], channels: u8, mut f: impl FnMut([f32; 2])) {
    const SCALE: f32 = 1.0 / 32768.0;
    let sample = |b: &[u8]| i16::from_le_bytes([b[0], b[1]]) as f32 * SCALE;
    if channels == 1 {
        for s in payload.chunks_exact(2) {
            let v = sample(s);
            f([v, v]);
        }
    } else {
        for s in payload.chunks_exact(4) {
            f([sample(&s[0..2]), sample(&s[2..4])]);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqVerdict {
    InOrder,
    /// This many packets went missing before this one.
    Gap(u32),
    /// Older than what we already played (reordered or duplicated) — drop it.
    Late,
    /// Discontinuity too large to be loss or reordering: a new stream started.
    Reset,
}

#[derive(Debug, Default)]
pub struct SeqTracker {
    expected: Option<u32>,
}

impl SeqTracker {
    const WINDOW: i64 = 1000;

    pub fn check(&mut self, seq: u32) -> SeqVerdict {
        let verdict = match self.expected {
            None => SeqVerdict::InOrder,
            Some(expected) => {
                let diff = seq.wrapping_sub(expected) as i32 as i64;
                if diff == 0 {
                    SeqVerdict::InOrder
                } else if diff > 0 && diff < Self::WINDOW {
                    SeqVerdict::Gap(diff as u32)
                } else if diff < 0 && diff > -Self::WINDOW {
                    return SeqVerdict::Late;
                } else {
                    SeqVerdict::Reset
                }
            }
        };
        self.expected = Some(seq.wrapping_add(1));
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_roundtrip() {
        let frames: Vec<[i16; 2]> = (0..FRAMES_PER_PACKET as i16).map(|i| [i, -i]).collect();
        let mut buf = Vec::new();
        encode_packet(&mut buf, 0xDEAD_BEEF, 42, 48_000, &frames);
        assert_eq!(buf.len(), MAX_PACKET_LEN);
        assert!(buf.len() < 1200, "packets must stay below the MTU");

        let (header, payload) = decode_header(&buf).unwrap();
        assert_eq!(header.session, 0xDEAD_BEEF);
        assert_eq!(header.seq, 42);
        assert_eq!(header.sample_rate, 48_000);
        assert_eq!(header.frames as usize, FRAMES_PER_PACKET);

        let mut decoded = Vec::new();
        for_each_frame(payload, header.channels, |f| decoded.push(f));
        assert_eq!(decoded.len(), FRAMES_PER_PACKET);
        assert_eq!(decoded[10], [10.0 / 32768.0, -10.0 / 32768.0]);
    }

    #[test]
    fn rejects_malformed_packets() {
        let mut buf = Vec::new();
        encode_packet(&mut buf, 1, 1, 48_000, &[[0, 0]; 4]);
        assert_eq!(decode_header(&buf[..10]), Err(DecodeError::TooShort));

        let mut bad_magic = buf.clone();
        bad_magic[0] = b'X';
        assert_eq!(decode_header(&bad_magic), Err(DecodeError::BadMagic));

        let mut truncated = buf.clone();
        truncated.pop();
        assert_eq!(decode_header(&truncated), Err(DecodeError::BadLength));

        let mut bad_codec = buf.clone();
        bad_codec[19] = 9;
        assert_eq!(decode_header(&bad_codec), Err(DecodeError::Unsupported));
    }

    #[test]
    fn sequence_tracking() {
        let mut t = SeqTracker::default();
        assert_eq!(t.check(u32::MAX - 1), SeqVerdict::InOrder);
        assert_eq!(t.check(u32::MAX), SeqVerdict::InOrder);
        assert_eq!(t.check(0), SeqVerdict::InOrder, "wraps around");
        assert_eq!(t.check(3), SeqVerdict::Gap(2));
        assert_eq!(t.check(2), SeqVerdict::Late);
        assert_eq!(
            t.check(4),
            SeqVerdict::InOrder,
            "late packet did not move the cursor"
        );
        assert_eq!(t.check(2_000_000), SeqVerdict::Reset);
        assert_eq!(t.check(2_000_001), SeqVerdict::InOrder);
    }
}
