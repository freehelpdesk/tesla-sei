# tesla-sei

Extract Tesla dashcam SEI (Supplemental Enhancement Information) telemetry from dashcam MP4 files.

This repo contains:
- A CLI binary (`tesla-sei`) for exporting telemetry to CSV or JSON.
- A reusable Rust library API for per-sample/per-frame streaming extraction.

## Install / Build

- Build: `cargo build`
- Run: `cargo run -- --help`

## CLI Usage

Basic:
- CSV to stdout:
  - `cargo run -- --csv /path/to/clip.mp4`
- CSV to a file:
  - `cargo run -- --csv /path/to/clip.mp4 -o telem.csv`
- JSON (pretty-printed array):
  - `cargo run -- --json /path/to/clip.mp4 -o telem.json`

Enum formatting:
- Print protobuf enums as string names (e.g. `GEAR_DRIVE`):
  - `cargo run -- --csv /path/to/clip.mp4 -e -o telem.csv`

Notes:
- `-o -` writes to stdout.
- `--format csv|json` is available; `--csv` and `--json` are convenience aliases.

## Library API

Add as a dependency (from a git checkout or a published crate):
- If local path dependency:
  - In your `Cargo.toml`: `tesla-sei = { path = "../tesla-sei" }`

### Sync (iterator) extraction

Use the iterator API to process events as they are decoded:

- `tesla_sei::extractor_from_path(...) -> SeiExtractor<File>`
- `SeiExtractor` implements `Iterator<Item = io::Result<SeiEvent>>`

### Async (Tokio) streaming

Async support is enabled by default.

- `tesla_sei::stream_from_path(path, buffer)` returns a Tokio `Stream` of `io::Result<SeiEvent>`.
- Internally it runs the sync extractor on a blocking thread and forwards events over a channel.

## Debugging MP4 parsing

Enable MP4 tracing:
- `TESLA_SEI_TRACE_MP4=1 cargo run -- --csv /path/to/clip.mp4`

## Output semantics

- The extractor iterates MP4 *samples* from the selected video track.
- Each sample may contain 0..N SEI messages.
- The main “frame identifier” in the protobuf is typically `frame_seq_no`.

## License

MIT
