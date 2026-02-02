use std::env;
use std::io::{self, Read, Seek, SeekFrom};

use crate::Error;

// -----------------------------
// MP4 parsing (minimal ISO-BMFF)
// -----------------------------
#[derive(Debug, Clone)]
pub(crate) struct TrackSampleTables {
    // stsz
    pub(crate) sample_sizes: Vec<u32>,
    // stco/co64
    pub(crate) chunk_offsets: Vec<u64>,
    // stsc
    pub(crate) stsc: Vec<StscEntry>,
    // codec config (avcC/hvcC)
    pub(crate) codec: CodecConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct StscEntry {
    pub(crate) first_chunk: u32,
    pub(crate) samples_per_chunk: u32,
    #[allow(dead_code)]
    pub(crate) sample_description_index: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum CodecConfig {
    Avc { nal_len_size: usize },  // from avcC lengthSizeMinusOne + 1
    Hevc { nal_len_size: usize }, // from hvcC (same idea)
    Unknown,
}

#[derive(Debug)]
pub(crate) struct Mp4 {
    pub(crate) tracks: Vec<TrackSampleTables>,
}

fn read_u8<R: Read>(r: &mut R) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_be_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn read_be_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}

#[derive(Debug, Clone)]
struct BoxHeader {
    typ: [u8; 4],
    size: u64,
    header_len: u64,
}

fn read_box_header<R: Read>(r: &mut R) -> io::Result<BoxHeader> {
    let size32 = read_be_u32(r)? as u64;
    let mut typ = [0u8; 4];
    r.read_exact(&mut typ)?;
    if size32 == 1 {
        // largesize
        let size64 = read_be_u64(r)?;
        Ok(BoxHeader {
            typ,
            size: size64,
            header_len: 16,
        })
    } else {
        Ok(BoxHeader {
            typ,
            size: size32,
            header_len: 8,
        })
    }
}

fn fourcc(s: &str) -> [u8; 4] {
    let b = s.as_bytes();
    [b[0], b[1], b[2], b[3]]
}

fn fourcc_to_string(t: [u8; 4]) -> String {
    // Best-effort display for debugging.
    t.iter()
        .map(|&c| if c.is_ascii_graphic() { c as char } else { '.' })
        .collect()
}

fn trace_enabled() -> bool {
    matches!(
        env::var("TESLA_SEI_TRACE_MP4").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn trace_box(ctx: &str, start: u64, hdr: &BoxHeader, limit: u64) {
    if trace_enabled() {
        eprintln!(
            "[mp4] {ctx}: pos={start} typ={} size={} header={} limit={}",
            fourcc_to_string(hdr.typ),
            hdr.size,
            hdr.header_len,
            limit
        );
    }
}

fn safe_box_end(ctx: &str, start: u64, hdr: &BoxHeader, limit: u64) -> Result<u64, Error> {
    // ISO-BMFF: size==0 means "extends to end of file" (or end of the containing box).
    let mut size = hdr.size;
    if size == 0 {
        size = limit.saturating_sub(start);
    }
    if size < hdr.header_len {
        return Err(Error::Mp4InvalidBox {
            context: ctx.to_string(),
            box_type: fourcc_to_string(hdr.typ),
            offset: start,
            message: format!("size {size} < header_len {}", hdr.header_len),
        });
    }

    let mut end = start.saturating_add(size);

    // Clamp to containing limit to avoid seeking past boundaries on malformed files.
    if end > limit {
        end = limit;
    }

    // Guarantee forward progress.
    if end <= start {
        return Err(Error::Mp4InvalidBox {
            context: ctx.to_string(),
            box_type: fourcc_to_string(hdr.typ),
            offset: start,
            message: format!("non-advancing end {end}"),
        });
    }

    Ok(end)
}

pub(crate) fn parse_mp4<R: Read + Seek>(f: &mut R) -> Result<Mp4, Error> {
    let mut tracks: Vec<TrackSampleTables> = Vec::new();

    let file_len = f.seek(SeekFrom::End(0))?;
    let mut pos = 0u64;

    // Walk top-level boxes, find moov
    while pos + 8 <= file_len {
        f.seek(SeekFrom::Start(pos))?;
        let hdr = read_box_header(f)?;
        let start = pos;
        trace_box("top", start, &hdr, file_len);
        let end = safe_box_end("top", start, &hdr, file_len)?;
        let payload_start = start + hdr.header_len;

        if hdr.typ == fourcc("moov") {
            // parse moov children
            parse_moov(f, payload_start, end, &mut tracks)?;
        }

        pos = end;
    }

    Ok(Mp4 { tracks })
}

fn parse_moov<R: Read + Seek>(
    f: &mut R,
    mut pos: u64,
    end: u64,
    tracks: &mut Vec<TrackSampleTables>,
) -> Result<(), Error> {
    while pos + 8 <= end {
        f.seek(SeekFrom::Start(pos))?;
        let hdr = read_box_header(f)?;
        let start = pos;
        trace_box("moov", start, &hdr, end);
        let box_end = safe_box_end("moov", start, &hdr, end)?;
        let payload_start = start + hdr.header_len;

        if hdr.typ == fourcc("trak") {
            if let Some(t) = parse_trak(f, payload_start, box_end)? {
                tracks.push(t);
            }
        }

        pos = box_end;
    }
    Ok(())
}

fn parse_trak<R: Read + Seek>(
    f: &mut R,
    mut pos: u64,
    end: u64,
) -> Result<Option<TrackSampleTables>, Error> {
    // We only care about video tracks. We'll detect by presence of stsd avc1/hvc1/etc.
    while pos + 8 <= end {
        f.seek(SeekFrom::Start(pos))?;
        let hdr = read_box_header(f)?;
        let start = pos;
        trace_box("trak", start, &hdr, end);
        let box_end = safe_box_end("trak", start, &hdr, end)?;
        let payload_start = start + hdr.header_len;

        if hdr.typ == fourcc("mdia") {
            return parse_mdia(f, payload_start, box_end);
        }

        pos = box_end;
    }
    Ok(None)
}

fn parse_mdia<R: Read + Seek>(f: &mut R, mut pos: u64, end: u64) -> Result<Option<TrackSampleTables>, Error> {
    let mut handler_type: Option<[u8; 4]> = None;
    let mut stbl_tables: Option<TrackSampleTables> = None;
    let mut minf_err: Option<Error> = None;

    while pos + 8 <= end {
        f.seek(SeekFrom::Start(pos))?;
        let hdr = read_box_header(f)?;
        let start = pos;
        trace_box("mdia", start, &hdr, end);
        let box_end = safe_box_end("mdia", start, &hdr, end)?;
        let payload_start = start + hdr.header_len;

        match hdr.typ {
            t if t == fourcc("hdlr") => {
                // hdlr: version/flags (4) + pre_defined (4) + handler_type (4)
                f.seek(SeekFrom::Start(payload_start + 8))?;
                let mut ht = [0u8; 4];
                f.read_exact(&mut ht)?;
                handler_type = Some(ht);
            }
            t if t == fourcc("minf") => {
                match parse_minf(f, payload_start, box_end) {
                    Ok(v) => stbl_tables = v,
                    Err(e) => minf_err = Some(e),
                }
            }
            _ => {}
        }

        pos = box_end;
    }

    // Keep only video handler 'vide'
    if handler_type == Some(fourcc("vide")) {
        if let Some(e) = minf_err {
            return Err(e);
        }
        Ok(stbl_tables)
    } else {
        Ok(None)
    }
}

fn parse_minf<R: Read + Seek>(f: &mut R, mut pos: u64, end: u64) -> Result<Option<TrackSampleTables>, Error> {
    while pos + 8 <= end {
        f.seek(SeekFrom::Start(pos))?;
        let hdr = read_box_header(f)?;
        let start = pos;
        trace_box("minf", start, &hdr, end);
        let box_end = safe_box_end("minf", start, &hdr, end)?;
        let payload_start = start + hdr.header_len;

        if hdr.typ == fourcc("stbl") {
            return parse_stbl(f, payload_start, box_end).map(Some);
        }

        pos = box_end;
    }
    Ok(None)
}

fn parse_stbl<R: Read + Seek>(f: &mut R, mut pos: u64, end: u64) -> Result<TrackSampleTables, Error> {
    let mut sample_sizes: Option<Vec<u32>> = None;
    let mut chunk_offsets: Option<Vec<u64>> = None;
    let mut stsc: Option<Vec<StscEntry>> = None;
    let mut codec: CodecConfig = CodecConfig::Unknown;

    while pos + 8 <= end {
        f.seek(SeekFrom::Start(pos))?;
        let hdr = read_box_header(f)?;
        let start = pos;
        trace_box("stbl", start, &hdr, end);
        let box_end = safe_box_end("stbl", start, &hdr, end)?;
        let payload_start = start + hdr.header_len;

        match hdr.typ {
            t if t == fourcc("stsd") => {
                codec = parse_stsd_for_codec(f, payload_start, box_end)?;
            }
            t if t == fourcc("stsz") => {
                sample_sizes = Some(parse_stsz(f, payload_start)?);
            }
            t if t == fourcc("stco") => {
                chunk_offsets = Some(parse_stco(f, payload_start)?);
            }
            t if t == fourcc("co64") => {
                chunk_offsets = Some(parse_co64(f, payload_start)?);
            }
            t if t == fourcc("stsc") => {
                stsc = Some(parse_stsc(f, payload_start)?);
            }
            _ => {}
        }

        pos = box_end;
    }

    let mut missing: Vec<&'static str> = Vec::new();
    if sample_sizes.is_none() {
        missing.push("stsz");
    }
    if chunk_offsets.is_none() {
        missing.push("stco/co64");
    }
    if stsc.is_none() {
        missing.push("stsc");
    }

    if !missing.is_empty() {
        return Err(Error::Mp4MissingSampleTables {
            missing: missing.join(", "),
        });
    }

    Ok(TrackSampleTables {
        sample_sizes: sample_sizes.unwrap(),
        chunk_offsets: chunk_offsets.unwrap(),
        stsc: stsc.unwrap(),
        codec,
    })
}

fn parse_stsz<R: Read + Seek>(f: &mut R, payload_start: u64) -> io::Result<Vec<u32>> {
    f.seek(SeekFrom::Start(payload_start))?;
    let _version_flags = read_be_u32(f)?;
    let sample_size = read_be_u32(f)?;
    let sample_count = read_be_u32(f)?;
    let mut sizes = Vec::with_capacity(sample_count as usize);

    if sample_size != 0 {
        sizes.resize(sample_count as usize, sample_size);
        return Ok(sizes);
    }

    for _ in 0..sample_count {
        sizes.push(read_be_u32(f)?);
    }
    Ok(sizes)
}

fn parse_stco<R: Read + Seek>(f: &mut R, payload_start: u64) -> io::Result<Vec<u64>> {
    f.seek(SeekFrom::Start(payload_start))?;
    let _version_flags = read_be_u32(f)?;
    let count = read_be_u32(f)?;
    let mut v = Vec::with_capacity(count as usize);
    for _ in 0..count {
        v.push(read_be_u32(f)? as u64);
    }
    Ok(v)
}

fn parse_co64<R: Read + Seek>(f: &mut R, payload_start: u64) -> io::Result<Vec<u64>> {
    f.seek(SeekFrom::Start(payload_start))?;
    let _version_flags = read_be_u32(f)?;
    let count = read_be_u32(f)?;
    let mut v = Vec::with_capacity(count as usize);
    for _ in 0..count {
        v.push(read_be_u64(f)?);
    }
    Ok(v)
}

fn parse_stsc<R: Read + Seek>(f: &mut R, payload_start: u64) -> io::Result<Vec<StscEntry>> {
    f.seek(SeekFrom::Start(payload_start))?;
    let _version_flags = read_be_u32(f)?;
    let count = read_be_u32(f)?;
    let mut v = Vec::with_capacity(count as usize);
    for _ in 0..count {
        v.push(StscEntry {
            first_chunk: read_be_u32(f)?,
            samples_per_chunk: read_be_u32(f)?,
            sample_description_index: read_be_u32(f)?,
        });
    }
    Ok(v)
}

fn parse_stsd_for_codec<R: Read + Seek>(
    f: &mut R,
    payload_start: u64,
    stsd_end: u64,
) -> Result<CodecConfig, Error> {
    // stsd: version/flags (4) + entry_count (4) + sample entries...
    f.seek(SeekFrom::Start(payload_start))?;
    let _version_flags = read_be_u32(f)?;
    let entry_count = read_be_u32(f)?;
    if entry_count == 0 {
        return Ok(CodecConfig::Unknown);
    }

    // sample entry is itself a box-ish structure: size + type
    let entry_pos = payload_start + 8;
    f.seek(SeekFrom::Start(entry_pos))?;
    let entry_size = read_be_u32(f)? as u64;
    let mut entry_type = [0u8; 4];
    f.read_exact(&mut entry_type)?;

    // We need avcC or hvcC inside this sample entry.
    // Sample entry has a fixed header (6 reserved + 2 data_ref_idx) etc.
    // We'll just scan child boxes within the entry payload for avcC/hvcC.
    let entry_start = entry_pos;
    let entry_payload_start = entry_pos + 8;
    let entry_end = if entry_size == 0 {
        stsd_end
    } else {
        (entry_start + entry_size).min(stsd_end)
    };

    // For video sample entries (avc1/hvc1/hev1), child boxes start after the fixed VisualSampleEntry header.
    // VisualSampleEntry is 78 bytes after the size+type header.
    let visual_sample_entry_len: u64 = 78;
    let mut p = match entry_type {
        t if t == fourcc("avc1") || t == fourcc("hvc1") || t == fourcc("hev1") => {
            entry_payload_start.saturating_add(visual_sample_entry_len)
        }
        _ => entry_payload_start,
    };
    if p > entry_end {
        p = entry_payload_start;
    }
    while p + 8 <= entry_end {
        f.seek(SeekFrom::Start(p))?;
        let hdr = read_box_header(f)?;
        let start = p;
        // Child boxes can also legally be size==0; treat as extending to end of sample entry.
        let child_end = safe_box_end("stsd", start, &hdr, entry_end)?;
        let payload = start + hdr.header_len;

        if hdr.typ == fourcc("avcC") {
            let nal = parse_avcc_nal_len(f, payload)?;
            return Ok(CodecConfig::Avc { nal_len_size: nal });
        }
        if hdr.typ == fourcc("hvcC") {
            let nal = parse_hvcc_nal_len(f, payload)?;
            return Ok(CodecConfig::Hevc { nal_len_size: nal });
        }

        p = child_end;
    }

    // fallback: still accept video even if unknown; try 4-byte NAL lengths
    Ok(match entry_type {
        t if t == fourcc("avc1") => CodecConfig::Avc { nal_len_size: 4 },
        t if t == fourcc("hvc1") || t == fourcc("hev1") => CodecConfig::Hevc { nal_len_size: 4 },
        _ => CodecConfig::Unknown,
    })
}

fn parse_avcc_nal_len<R: Read + Seek>(f: &mut R, payload_start: u64) -> io::Result<usize> {
    // avcC:
    // configurationVersion(1), AVCProfileIndication(1), profile_compat(1), AVCLevelIndication(1),
    // lengthSizeMinusOne in low 2 bits of next byte
    f.seek(SeekFrom::Start(payload_start + 4))?;
    let b = read_u8(f)?;
    let len_minus_one = (b & 0b11) as usize;
    Ok(len_minus_one + 1)
}

fn parse_hvcc_nal_len<R: Read + Seek>(f: &mut R, payload_start: u64) -> io::Result<usize> {
    // hvcC: we only need lengthSizeMinusOne which is near the end of the fixed header.
    // Per ISO/IEC 14496-15, lengthSizeMinusOne is in the byte after:
    // configurationVersion (1) + ... + min_spatial_segmentation_idc(2) + parallelismType(1)
    // + chromaFormat(1) + bitDepthLumaMinus8(1) + bitDepthChromaMinus8(1)
    // + avgFrameRate(2) + constantFrameRate/numTemporalLayers/temporalIdNested (1)
    // then lengthSizeMinusOne (2 bits) in next byte.
    // That lands at offset 21 (0-based) for the common layout.
    f.seek(SeekFrom::Start(payload_start + 21))?;
    let b = read_u8(f)?;
    let len_minus_one = (b & 0b11) as usize;
    Ok(len_minus_one + 1)
}

// Turn stsc + stco + stsz into per-sample absolute file offsets.
pub(crate) fn build_sample_offsets(t: &TrackSampleTables) -> Result<Vec<u64>, Error> {
    // Expand chunk -> samples_per_chunk using stsc runs.
    // MP4 chunks are 1-based in stsc.
    let mut chunk_samples: Vec<u32> = vec![0; t.chunk_offsets.len()];

    for i in 0..t.stsc.len() {
        let cur = &t.stsc[i];
        let next_first = t
            .stsc
            .get(i + 1)
            .map(|e| e.first_chunk)
            .unwrap_or((t.chunk_offsets.len() as u32) + 1);

        for chunk_idx_1based in cur.first_chunk..next_first {
            let idx0 = (chunk_idx_1based - 1) as usize;
            if idx0 < chunk_samples.len() {
                chunk_samples[idx0] = cur.samples_per_chunk;
            }
        }
    }

    // Some files can be slightly malformed (or we parsed an unexpected stsc ordering).
    // Fill any zeros with the previous non-zero value so we still walk all chunks.
    let mut last = 0u32;
    for v in &mut chunk_samples {
        if *v == 0 {
            *v = last;
        } else {
            last = *v;
        }
    }

    // Now compute offsets by walking chunks in order.
    let mut sample_offsets = Vec::with_capacity(t.sample_sizes.len());
    let mut sample_index = 0usize;

    for (chunk_i, &chunk_off) in t.chunk_offsets.iter().enumerate() {
        let spc = chunk_samples[chunk_i] as usize;
        let mut off = chunk_off;

        for _ in 0..spc {
            if sample_index >= t.sample_sizes.len() {
                break;
            }
            sample_offsets.push(off);
            off += t.sample_sizes[sample_index] as u64;
            sample_index += 1;
        }
    }

    if sample_offsets.len() != t.sample_sizes.len() {
        return Err(Error::Mp4InconsistentSampleTables {
            sample_sizes: t.sample_sizes.len(),
            sample_offsets: sample_offsets.len(),
            chunk_offsets: t.chunk_offsets.len(),
        });
    }

    Ok(sample_offsets)
}
