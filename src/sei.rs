use prost::Message;

use crate::mp4::CodecConfig;
use crate::pb;

// -----------------------------
// NAL + SEI parsing
// -----------------------------
fn split_nals_length_prefixed(sample: &[u8], nal_len_size: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + nal_len_size <= sample.len() {
        let len = match nal_len_size {
            1 => sample[i] as usize,
            2 => u16::from_be_bytes([sample[i], sample[i + 1]]) as usize,
            3 => ((sample[i] as usize) << 16)
                | ((sample[i + 1] as usize) << 8)
                | (sample[i + 2] as usize),
            4 => u32::from_be_bytes([sample[i], sample[i + 1], sample[i + 2], sample[i + 3]])
                as usize,
            _ => break,
        };
        i += nal_len_size;
        if i + len > sample.len() || len == 0 {
            break;
        }
        out.push(&sample[i..i + len]);
        i += len;
    }
    out
}

fn remove_emulation_prevention(rbsp: &[u8]) -> Vec<u8> {
    // Remove 0x03 after 0x00 0x00 sequences (H264/H265)
    let mut out = Vec::with_capacity(rbsp.len());
    let mut i = 0usize;
    let mut zeros = 0usize;

    while i < rbsp.len() {
        let b = rbsp[i];
        if zeros >= 2 && b == 0x03 {
            // skip this emulation prevention byte
            i += 1;
            zeros = 0;
            continue;
        }
        out.push(b);
        if b == 0x00 {
            zeros += 1;
        } else {
            zeros = 0;
        }
        i += 1;
    }
    out
}

fn parse_sei_messages(rbsp: &[u8]) -> Vec<(u32, Vec<u8>)> {
    // Returns (payload_type, payload_bytes)
    let data = remove_emulation_prevention(rbsp);
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < data.len() {
        // payloadType
        let mut payload_type: u32 = 0;
        while i < data.len() && data[i] == 0xFF {
            payload_type += 255;
            i += 1;
        }
        if i >= data.len() {
            break;
        }
        payload_type += data[i] as u32;
        i += 1;

        // payloadSize
        let mut payload_size: usize = 0;
        while i < data.len() && data[i] == 0xFF {
            payload_size += 255;
            i += 1;
        }
        if i >= data.len() {
            break;
        }
        payload_size += data[i] as usize;
        i += 1;

        if i + payload_size > data.len() {
            break;
        }
        let payload = data[i..i + payload_size].to_vec();
        i += payload_size;

        out.push((payload_type, payload));

        // rbsp_trailing_bits follow; we can just stop if remaining is tiny
        if data.len().saturating_sub(i) <= 1 {
            break;
        }
    }

    out
}

fn try_decode_sei_metadata_from_payload(payload_type: u32, payload: &[u8]) -> Option<pb::SeiMetadata> {
    // Tesla often uses user_data_unregistered (type 5) which typically starts with a 16-byte UUID.
    // Some files may include additional header bytes; we try a small set of plausible offsets.
    //
    // IMPORTANT: protobuf decode of an empty slice is valid and yields an all-defaults message.
    // If we accidentally pass an empty slice (e.g., UUID-only payload), we emit bogus rows.
    let mut candidates: Vec<&[u8]> = Vec::new();

    // Tesla's JS looks for a magic prefix of 0x42 bytes followed by 0x69, then decodes the bytes
    // after that marker. Implement that first to avoid false positives.
    if payload_type == 5 {
        let mut i = 0usize;
        while i < payload.len() && payload[i] == 0x42 {
            i += 1;
        }
        if i > 0 && i < payload.len() && payload[i] == 0x69 {
            let start = i + 1;
            if start < payload.len() {
                candidates.push(&payload[start..]);
            }
        }
    }

    // Try skipping UUID for type 5.
    // NOTE: payload.len()==16 means UUID only; decoding an empty slice yields a default protobuf.
    if payload_type == 5 && payload.len() > 16 {
        candidates.push(&payload[16..]);
    }

    // Always try the payload as-is (fallback).
    if !payload.is_empty() {
        candidates.push(payload);
    }

    // Heuristic: protobuf messages often start with tag 0x08 (field 1, varint).
    let scan_len = payload.len().min(64);
    for i in 0..scan_len {
        if payload[i] == 0x08 && i + 2 <= payload.len() {
            candidates.push(&payload[i..]);
        }
    }

    // Deduplicate by pointer+len to avoid repeated decode attempts.
    candidates.dedup_by(|a, b| a.as_ptr() == b.as_ptr() && a.len() == b.len());

    for cand in candidates {
        if cand.is_empty() {
            continue;
        }

        // Tesla's JS drops the last byte because it doesn't parse payloadSize.
        // Our parser should already exclude rbsp_trailing_bits, but some payloads still appear to
        // carry a trailing stop bit; try both when present.
        let mut decode_attempts: [&[u8]; 2] = [cand, &[]];
        let mut attempt_count = 1usize;
        if cand.len() > 1 && cand[cand.len() - 1] == 0x80 {
            decode_attempts[1] = &cand[..cand.len() - 1];
            attempt_count = 2;
        }

        for attempt in decode_attempts.into_iter().take(attempt_count) {
            if attempt.is_empty() {
                continue;
            }

            if let Ok(msg) = pb::SeiMetadata::decode(attempt) {
                // Guard against false-positives: empty payloads decode as an all-defaults message.
                if msg.version == 0 && msg.frame_seq_no == 0 {
                    continue;
                }
                return Some(msg);
            }
        }
    }

    None
}

// Identify SEI NALs and decode protobufs.
pub(crate) fn decode_sei_from_sample(codec: &CodecConfig, sample: &[u8]) -> Vec<pb::SeiMetadata> {
    let nal_len_size = match codec {
        CodecConfig::Avc { nal_len_size } => *nal_len_size,
        CodecConfig::Hevc { nal_len_size } => *nal_len_size,
        _ => 4,
    };

    let nals = split_nals_length_prefixed(sample, nal_len_size);
    let mut out = Vec::new();

    for nal in nals {
        if nal.is_empty() {
            continue;
        }

        match codec {
            CodecConfig::Avc { .. } => {
                let nal_type = nal[0] & 0x1F;
                if nal_type != 6 {
                    continue;
                }
                // NAL header is 1 byte for H.264
                let rbsp = &nal[1..];
                for (pt, pl) in parse_sei_messages(rbsp) {
                    if let Some(msg) = try_decode_sei_metadata_from_payload(pt, &pl) {
                        out.push(msg);
                    }
                }
            }
            CodecConfig::Hevc { .. } => {
                if nal.len() < 2 {
                    continue;
                }
                // HEVC nal_unit_type: bits 1..6 of first byte
                let nal_type = (nal[0] >> 1) & 0x3F;
                if nal_type != 39 && nal_type != 40 {
                    continue; // prefix/suffix SEI
                }
                // HEVC NAL header is 2 bytes
                let rbsp = &nal[2..];
                for (pt, pl) in parse_sei_messages(rbsp) {
                    if let Some(msg) = try_decode_sei_metadata_from_payload(pt, &pl) {
                        out.push(msg);
                    }
                }
            }
            _ => {}
        }
    }

    out
}
