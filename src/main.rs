use clap::{Parser, ValueEnum};
use serde::Serialize;
use serde_json::{Number, Value};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use tesla_sei::extract;
use tesla_sei::pb;

#[derive(Debug, Serialize)]
struct Sei {
    version: u32,
    gear_state: Value,
    frame_seq_no: u64,
    vehicle_speed_mps: f32,
    accelerator_pedal_position: f32,
    steering_wheel_angle: f32,
    blinker_on_left: bool,
    blinker_on_right: bool,
    brake_applied: bool,
    autopilot_state: Value,
    latitude_deg: f64,
    longitude_deg: f64,
    heading_deg: f64,
    linear_acceleration_mps2_x: f64,
    linear_acceleration_mps2_y: f64,
    linear_acceleration_mps2_z: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Json,
    Csv,
}

fn sei_csv_header() -> &'static str {
    "version,gear_state,frame_seq_no,vehicle_speed_mps,accelerator_pedal_position,steering_wheel_angle,blinker_on_left,blinker_on_right,brake_applied,autopilot_state,latitude_deg,longitude_deg,heading_deg,linear_acceleration_mps2_x,linear_acceleration_mps2_y,linear_acceleration_mps2_z"
}

#[derive(Parser, Debug)]
#[command(name = "tesla-sei")]
#[command(about = "Extract Tesla dashcam SEI metadata", long_about = None)]
struct Cli {
    /// Input MP4 file
    #[arg(value_name = "INPUT.mp4")]
    input: PathBuf,

    /// Output file path (use '-' for stdout)
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    /// Output format
    #[arg(long = "format", value_enum, default_value_t = OutputFormat::Json, conflicts_with_all = ["csv", "json"])]
    format: OutputFormat,

    /// Alias for `--format csv`
    #[arg(long, conflicts_with_all = ["json", "format"], action = clap::ArgAction::SetTrue)]
    csv: bool,

    /// Alias for `--format json`
    #[arg(long, conflicts_with_all = ["csv", "format"], action = clap::ArgAction::SetTrue)]
    json: bool,

    /// Print protobuf enums as their string names (e.g. GEAR_DRIVE) instead of numeric values
    #[arg(short = 'e', long = "enum", action = clap::ArgAction::SetTrue)]
    enum_strings: bool,
}

fn resolve_format(cli: &Cli) -> OutputFormat {
    if cli.csv {
        OutputFormat::Csv
    } else if cli.json {
        OutputFormat::Json
    } else {
        cli.format
    }
}

fn should_write_to_stdout(output: &Option<PathBuf>) -> bool {
    match output {
        None => true,
        Some(p) => p.as_os_str() == "-",
    }
}

fn gear_state_string(v: i32) -> String {
    match pb::sei_metadata::Gear::try_from(v) {
        Ok(e) => e.as_str_name().to_string(),
        Err(_) => format!("UNKNOWN({v})"),
    }
}

fn autopilot_state_string(v: i32) -> String {
    match pb::sei_metadata::AutopilotState::try_from(v) {
        Ok(e) => e.as_str_name().to_string(),
        Err(_) => format!("UNKNOWN({v})"),
    }
}

fn fmt_f32(v: f32) -> String {
    // Print with high decimal precision for downstream ML/analysis.
    // Cast to f64 to expose the exact stored f32 value (common desire for telemetry).
    format!("{:.15}", v as f64)
}

fn fmt_f64(v: f64) -> String {
    format!("{:.15}", v)
}

impl From<pb::SeiMetadata> for Sei {
    fn from(m: pb::SeiMetadata) -> Self {
        Sei {
            version: m.version,
            gear_state: Value::Number(Number::from(m.gear_state)),
            frame_seq_no: m.frame_seq_no,
            vehicle_speed_mps: m.vehicle_speed_mps,
            accelerator_pedal_position: m.accelerator_pedal_position,
            steering_wheel_angle: m.steering_wheel_angle,
            blinker_on_left: m.blinker_on_left,
            blinker_on_right: m.blinker_on_right,
            brake_applied: m.brake_applied,
            autopilot_state: Value::Number(Number::from(m.autopilot_state)),
            latitude_deg: m.latitude_deg,
            longitude_deg: m.longitude_deg,
            heading_deg: m.heading_deg,
            linear_acceleration_mps2_x: m.linear_acceleration_mps2_x,
            linear_acceleration_mps2_y: m.linear_acceleration_mps2_y,
            linear_acceleration_mps2_z: m.linear_acceleration_mps2_z,
        }
    }
}

impl Sei {
    fn from_pb(m: pb::SeiMetadata, enum_strings: bool) -> Self {
        if !enum_strings {
            return m.into();
        }

        Sei {
            version: m.version,
            gear_state: Value::String(gear_state_string(m.gear_state)),
            frame_seq_no: m.frame_seq_no,
            vehicle_speed_mps: m.vehicle_speed_mps,
            accelerator_pedal_position: m.accelerator_pedal_position,
            steering_wheel_angle: m.steering_wheel_angle,
            blinker_on_left: m.blinker_on_left,
            blinker_on_right: m.blinker_on_right,
            brake_applied: m.brake_applied,
            autopilot_state: Value::String(autopilot_state_string(m.autopilot_state)),
            latitude_deg: m.latitude_deg,
            longitude_deg: m.longitude_deg,
            heading_deg: m.heading_deg,
            linear_acceleration_mps2_x: m.linear_acceleration_mps2_x,
            linear_acceleration_mps2_y: m.linear_acceleration_mps2_y,
            linear_acceleration_mps2_z: m.linear_acceleration_mps2_z,
        }
    }
}

fn run_with_writer(
    input: &PathBuf,
    format: OutputFormat,
    enum_strings: bool,
    out: &mut dyn Write,
) -> io::Result<()> {
    let extractor = extract::extractor_from_path(input)?;

    let mut results: Vec<Sei> = Vec::new();

    if format == OutputFormat::Csv {
        writeln!(out, "{}", sei_csv_header())?;
    }

    for event in extractor {
        let msg = event?.metadata;
        match format {
            OutputFormat::Json => results.push(Sei::from_pb(msg, enum_strings)),
            OutputFormat::Csv => {
                let gear = if enum_strings {
                    gear_state_string(msg.gear_state)
                } else {
                    msg.gear_state.to_string()
                };
                let autopilot = if enum_strings {
                    autopilot_state_string(msg.autopilot_state)
                } else {
                    msg.autopilot_state.to_string()
                };

                // Write rows as we go (lower memory, easy to stream).
                // NB: we avoid quoting because values are numeric/bool/enum tokens.
                writeln!(
                    out,
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                    msg.version,
                    gear,
                    msg.frame_seq_no,
                    fmt_f32(msg.vehicle_speed_mps),
                    fmt_f32(msg.accelerator_pedal_position),
                    fmt_f32(msg.steering_wheel_angle),
                    msg.blinker_on_left,
                    msg.blinker_on_right,
                    msg.brake_applied,
                    autopilot,
                    fmt_f64(msg.latitude_deg),
                    fmt_f64(msg.longitude_deg),
                    fmt_f64(msg.heading_deg),
                    fmt_f64(msg.linear_acceleration_mps2_x),
                    fmt_f64(msg.linear_acceleration_mps2_y),
                    fmt_f64(msg.linear_acceleration_mps2_z)
                )?;
            }
        }
    }

    if format == OutputFormat::Json {
        let json = serde_json::to_string_pretty(&results).unwrap();
        writeln!(out, "{json}")?;
    }

    Ok(())
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let format = resolve_format(&cli);

    if should_write_to_stdout(&cli.output) {
        let stdout = io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        run_with_writer(&cli.input, format, cli.enum_strings, &mut out)?;
        out.flush()?;
    } else {
        let path = cli.output.as_ref().unwrap();
        let file = File::create(path)?;
        let mut out = BufWriter::new(file);
        run_with_writer(&cli.input, format, cli.enum_strings, &mut out)?;
        out.flush()?;
    }

    Ok(())
}