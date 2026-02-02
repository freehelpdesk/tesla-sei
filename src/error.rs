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
}
