use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::mp4::{build_sample_offsets, parse_mp4, CodecConfig, TrackSampleTables};
use crate::pb;
use crate::sei::decode_sei_from_sample;
use crate::Error;

/// A single decoded SEI telemetry event.
///
/// One MP4 sample may contain zero, one, or multiple SEI payloads; each decoded payload is
/// surfaced as a separate `SeiEvent`.
#[derive(Debug, Clone)]
pub struct SeiEvent {
    /// The 0-based sample index in the selected track.
    pub sample_index: usize,
    /// Absolute file offset where the MP4 sample begins.
    pub file_offset: u64,
    /// The decoded protobuf message.
    pub metadata: pb::SeiMetadata,
}

/// Streaming extractor that yields per-sample/per-frame telemetry as it is decoded.
///
/// This type is synchronous and requires a seekable input (`Read + Seek`). It implements
/// `Iterator<Item = Result<SeiEvent, Error>>`.
pub struct SeiExtractor<R: Read + Seek> {
    reader: R,
    sample_sizes: Vec<u32>,
    sample_offsets: Vec<u64>,
    codec: CodecConfig,

    next_sample_index: usize,
    pending_offset: u64,
    pending_sample_index: usize,
    pending: VecDeque<pb::SeiMetadata>,
}

/// Create an extractor from an on-disk MP4 path.
pub fn extractor_from_path(path: impl AsRef<Path>) -> Result<SeiExtractor<File>, Error> {
    let file = File::open(path)?;
    extractor_from_reader(file)
}

/// Create an extractor from any seekable reader.
///
/// This is the most flexible entry point for integrating into other Rust projects.
pub fn extractor_from_reader<R: Read + Seek>(mut reader: R) -> Result<SeiExtractor<R>, Error> {
    let mp4 = parse_mp4(&mut reader)?;

    if mp4.tracks.is_empty() {
        return Err(Error::NoTracksFound);
    }

    // Tesla clips sometimes contain multiple video tracks (e.g., a tiny preview track).
    // Pick the track with the most samples.
    let (_track_index, track) = mp4
        .tracks
        .iter()
        .enumerate()
        .max_by_key(|(_, t)| t.sample_sizes.len())
        .unwrap();

    let sample_offsets = build_sample_offsets(track)?;

    Ok(SeiExtractor {
        reader,
        sample_sizes: track.sample_sizes.clone(),
        sample_offsets,
        codec: track.codec.clone(),
        next_sample_index: 0,
        pending_offset: 0,
        pending_sample_index: 0,
        pending: VecDeque::new(),
    })
}

impl<R: Read + Seek> SeiExtractor<R> {
    /// Total number of MP4 samples in the selected track.
    pub fn total_samples(&self) -> usize {
        self.sample_offsets.len()
    }

    /// Pull the next event (convenience wrapper around `Iterator::next`).
    pub fn next_event(&mut self) -> Result<Option<SeiEvent>, Error> {
        self.next().transpose()
    }

    /// Seek the extractor so the next decoded events come from `sample_index`.
    ///
    /// This is useful for GUI "scrubbing" where you want to jump to an arbitrary point and
    /// then iterate forward.
    pub fn seek_sample(&mut self, sample_index: usize) -> Result<(), Error> {
        // Allow seeking to exactly `total_samples()` to position at EOF (iterator will return None).
        if sample_index > self.sample_offsets.len() {
            return Err(Error::SampleIndexOutOfRange {
                sample_index,
                total_samples: self.sample_offsets.len(),
            });
        }

        self.next_sample_index = sample_index;
        self.pending.clear();
        self.pending_offset = 0;
        self.pending_sample_index = 0;
        Ok(())
    }

    /// Decode telemetry events for an arbitrary `sample_index` without changing the iterator
    /// cursor.
    ///
    /// This is typically the most convenient API for GUI scrubbing: call this as the user drags
    /// a slider, and render the returned metadata.
    pub fn read_sample_events(&mut self, sample_index: usize) -> Result<Vec<SeiEvent>, Error> {
        let total = self.sample_offsets.len();
        if sample_index >= total {
            return Err(Error::SampleIndexOutOfRange {
                sample_index,
                total_samples: total,
            });
        }

        let off = self.sample_offsets[sample_index];
        let sz = self.sample_sizes[sample_index] as usize;
        let mut buf = vec![0u8; sz];
        self.reader.seek(SeekFrom::Start(off))?;
        self.reader.read_exact(&mut buf)?;

        let decoded = decode_sei_from_sample(&self.codec, &buf);
        let events = decoded
            .into_iter()
            .map(|metadata| SeiEvent {
                sample_index,
                file_offset: off,
                metadata,
            })
            .collect();

        Ok(events)
    }

    fn read_next_sample_into_pending(&mut self) -> Result<bool, Error> {
        while self.pending.is_empty() && self.next_sample_index < self.sample_offsets.len() {
            let sample_index = self.next_sample_index;
            let off = self.sample_offsets[sample_index];
            let sz = self.sample_sizes[sample_index] as usize;

            let mut buf = vec![0u8; sz];
            self.reader.seek(SeekFrom::Start(off))?;
            self.reader.read_exact(&mut buf)?;

            self.next_sample_index += 1;

            let decoded = decode_sei_from_sample(&self.codec, &buf);
            if decoded.is_empty() {
                continue;
            }

            self.pending_offset = off;
            self.pending_sample_index = sample_index;
            self.pending = decoded.into();
            return Ok(true);
        }

        Ok(!self.pending.is_empty())
    }
}

impl<R: Read + Seek> Iterator for SeiExtractor<R> {
    type Item = Result<SeiEvent, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Err(e) = self.read_next_sample_into_pending() {
            return Some(Err(e));
        }

        let metadata = self.pending.pop_front()?;
        Some(Ok(SeiEvent {
            sample_index: self.pending_sample_index,
            file_offset: self.pending_offset,
            metadata,
        }))
    }
}

/// Convenience helper that iterates all decoded events and invokes a callback.
///
/// This can be more ergonomic than manually writing a `for` loop when integrating in apps.
pub fn for_each_sei_metadata<R: Read + Seek>(
    reader: R,
    mut f: impl FnMut(SeiEvent) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut extractor = extractor_from_reader(reader)?;
    for event in &mut extractor {
        f(event?)?;
    }
    Ok(())
}

// Keep this around for future improvements, such as exposing track selection options.
#[allow(dead_code)]
fn _select_largest_track<'a>(tracks: &'a [TrackSampleTables]) -> Option<(usize, &'a TrackSampleTables)> {
    tracks
        .iter()
        .enumerate()
        .max_by_key(|(_, t)| t.sample_sizes.len())
}
