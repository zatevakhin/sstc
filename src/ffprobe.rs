use serde::{Deserialize, Serialize};
use std::error::Error;
use std::path::Path;
use std::process::Command;

fn parse_duration<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse::<f32>().map_err(serde::de::Error::custom)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FFprobeOutput {
    format: Format,
    // Other fields like streams, chapters, etc. can be added if needed
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Format {
    pub filename: String,
    pub nb_streams: u32,
    pub nb_programs: u32,
    pub nb_stream_groups: u32,
    pub format_name: String,
    pub format_long_name: String,
    pub start_time: String,
    #[serde(deserialize_with = "parse_duration")]
    pub duration: f32,
    pub size: String,
    pub bit_rate: String,
    pub probe_score: u32,
    pub tags: Option<Tags>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Tags {
    pub ENCODER: Option<String>,
    // Add other potential tags here
}

pub fn get_format_info<P: AsRef<Path>>(file_path: P) -> Result<Format, Box<dyn Error>> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-i",
            file_path.as_ref().to_str().ok_or("Invalid path")?,
        ])
        .output()?;

    if !output.status.success() {
        return Err(format!("ffprobe failed with status: {}", output.status).into());
    }

    let stdout = String::from_utf8(output.stdout)?;
    let ffprobe_data: FFprobeOutput = serde_json::from_str(&stdout)?;

    Ok(ffprobe_data.format)
}

