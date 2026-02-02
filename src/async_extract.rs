#![cfg(feature = "async")]

use std::io::{Read, Seek};
use std::path::PathBuf;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::extract::{extractor_from_path, extractor_from_reader, SeiEvent};
use crate::Error;

/// Create a Tokio `Stream` of per-sample/per-frame SEI events from an MP4 file on disk.
///
/// This API is enabled by default (crate feature `async`).
///
/// Implementation detail: MP4 extraction requires `Seek`, so this function runs the synchronous
/// extractor on a blocking thread (`tokio::task::spawn_blocking`) and forwards events over a
/// bounded channel.
///
/// `buffer` controls the channel capacity. Larger buffers can improve throughput if the consumer
/// occasionally stalls.
pub fn stream_from_path(
    path: impl Into<PathBuf>,
    buffer: usize,
) -> ReceiverStream<Result<SeiEvent, Error>> {
    let path = path.into();
    let (tx, rx) = mpsc::channel(buffer.max(1));

    tokio::task::spawn_blocking(move || {
        let mut extractor = match extractor_from_path(&path) {
            Ok(e) => e,
            Err(err) => {
                let _ = tx.blocking_send(Err(err));
                return;
            }
        };

        for item in &mut extractor {
            if tx.blocking_send(item).is_err() {
                break;
            }
        }
    });

    ReceiverStream::new(rx)
}

/// Create a Tokio `Stream` of per-sample/per-frame SEI events from any seekable reader.
///
/// This is useful for integration into other Rust projects that already manage IO.
///
/// The reader must be `Send + 'static` because extraction runs in `spawn_blocking`.
pub fn stream_from_reader<R>(reader: R, buffer: usize) -> ReceiverStream<Result<SeiEvent, Error>>
where
    R: Read + Seek + Send + 'static,
{
    let (tx, rx) = mpsc::channel(buffer.max(1));

    tokio::task::spawn_blocking(move || {
        let mut extractor = match extractor_from_reader(reader) {
            Ok(e) => e,
            Err(err) => {
                let _ = tx.blocking_send(Err(err));
                return;
            }
        };

        for item in &mut extractor {
            if tx.blocking_send(item).is_err() {
                break;
            }
        }
    });

    ReceiverStream::new(rx)
}
