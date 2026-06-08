use serde::Serialize;
use std::io::Write;

/// Dataset metadata matching the Python ecosystem format.
#[derive(Serialize)]
pub struct DatasetMetadata {
    pub generator: String,
    pub domain: String,
    pub scenario: String,
    pub trajectories: usize,
    pub physics_engine: String,
    pub version: String,
    pub generated_at: String,
}

/// A single trajectory record.
#[derive(Serialize)]
pub struct TrajectoryRecord {
    pub id: String,
    #[serde(rename = "type")]
    pub traj_type: String,
    pub scenario: String,
    pub steps: usize,
    pub score: serde_json::Value,
    pub proof_hash: String,
    pub reasoning_context: serde_json::Value,
    pub data: Vec<serde_json::Value>,
}

/// Complete dataset output.
#[derive(Serialize)]
pub struct Dataset {
    pub dataset_metadata: DatasetMetadata,
    pub trajectories: Vec<TrajectoryRecord>,
}

/// Generate an ISO 8601 timestamp string.
pub fn now_iso() -> String {
    // Manual UTC timestamp without chrono dependency
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Convert to rough ISO format
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Approximate year/month/day from epoch days
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining < md as i64 { m = i + 1; break; }
        remaining -= md as i64;
    }
    let d = remaining + 1;

    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hours, minutes, seconds)
}

/// Generate a short hex ID from the RNG
pub fn short_id(rng: &mut crate::rng::Rng) -> String {
    format!("{:08x}", rng.next_u64() as u32)
}

/// Write a dataset to a JSON file, streaming to avoid holding everything in memory.
pub fn write_dataset(path: &str, dataset: &Dataset) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path)?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, dataset)?;
    writer.flush()?;
    Ok(())
}

use std::fs::File;
use std::io::BufWriter;

/// Memory-safe streaming JSON writer for huge 10M+ trajectory lists.
pub struct DatasetStreamer {
    writer: BufWriter<File>,
    first_trajectory_written: bool,
}

impl DatasetStreamer {
    pub fn new(path: &str, metadata: &DatasetMetadata) -> std::io::Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        
        let meta_json = serde_json::to_string_pretty(metadata)
            .expect("Failed to serialize metadata");
        
        writeln!(&mut writer, "{{")?;
        writeln!(&mut writer, "  \"dataset_metadata\": {},", meta_json)?;
        writeln!(&mut writer, "  \"trajectories\": [")?;

        Ok(Self {
            writer,
            first_trajectory_written: false,
        })
    }

    pub fn write_trajectory(&mut self, record: &TrajectoryRecord) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(record)
            .expect("Failed to serialize record");
        
        // Indent the JSON block for pretty output
        let json: String = json.lines().map(|l| format!("    {}", l)).collect::<Vec<_>>().join("\n");

        if self.first_trajectory_written {
            writeln!(&mut self.writer, ",\n{}", json)?;
        } else {
            writeln!(&mut self.writer, "{}", json)?;
            self.first_trajectory_written = true;
        }

        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        writeln!(&mut self.writer, "\n  ]")?;
        writeln!(&mut self.writer, "}}")?;
        self.writer.flush()?;
        Ok(())
    }
}
