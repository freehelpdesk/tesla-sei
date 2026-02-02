use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// Passthrough for IO errors (open/read/seek).
    #[error(transparent)]
    Io(#[from] io::Error),

    /// No usable video track/sample tables were found in the MP4.
    #[error("no video tracks with sample tables found")]
    NoTracksFound,

    /// MP4 structure is malformed or violates expected ISO-BMFF invariants.
    #[error("mp4 parse error in {context}: box {box_type} at offset {offset}: {message}")]
    Mp4InvalidBox {
        context: String,
        box_type: String,
        offset: u64,
        message: String,
    },

    /// Required tables/structures for extraction are missing.
    #[error("mp4 missing required sample tables: {missing}")]
    Mp4MissingSampleTables { missing: String },

    /// MP4 sample tables are internally inconsistent.
    #[error(
        "mp4 inconsistent sample tables: sample_sizes={sample_sizes} derived_offsets={sample_offsets} chunk_offsets={chunk_offsets}"
    )]
    Mp4InconsistentSampleTables {
        sample_sizes: usize,
        sample_offsets: usize,
        chunk_offsets: usize,
    },

    /// Requested sample index is outside the available range.
    #[error("sample index out of range: {sample_index} (total_samples={total_samples})")]
    SampleIndexOutOfRange {
        sample_index: usize,
        total_samples: usize,
    },
}
