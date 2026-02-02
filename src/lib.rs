//! `tesla-sei` extracts Tesla dashcam SEI telemetry from MP4 files.
//!
//! This crate provides:
//! - A synchronous iterator-based extractor (good for scripts and simple pipelines).
//! - A Tokio-based async `Stream` wrapper (enabled by default) for easy integration with async apps.
//!
//! The primary payload type is the generated protobuf [`pb::SeiMetadata`].
//!
//! ## Quick start (sync)
//! - Open a file and iterate decoded events:
//!   - Use [`extractor_from_path`] and iterate the returned [`SeiExtractor`].
//!
//! ## Quick start (async)
//! - Use [`stream_from_path`] to get a Tokio `Stream` of events.
//!
//! ## Features
//! - `async` (default): enables Tokio stream helpers.

pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/dashcam.rs"));
}

pub mod error;

mod mp4;
mod sei;

pub mod extract;

#[cfg(feature = "async")]
pub mod async_extract;

pub use extract::{
    extractor_from_path, extractor_from_reader, for_each_sei_metadata, SeiEvent, SeiExtractor,
};

pub use error::Error;

#[cfg(feature = "async")]
pub use async_extract::{stream_from_path, stream_from_reader};
