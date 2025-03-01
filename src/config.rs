use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub inputs: Vec<InputConfig>,
    pub outputs: HashMap<String, OutputConfig>,
    pub presets: HashMap<String, PresetConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    pub path: PathBuf,
    pub extensions: Vec<String>,
    pub preset: String,
    pub output: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    pub path: PathBuf,
    pub filename_template: String,
    pub container: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PresetConfig {
    pub video_codec: String,
    pub audio_codec: String,
    pub video_bitrate: String,
    pub audio_bitrate: String,
    pub scale: String,
    pub extra_options: HashMap<String, String>,
}

pub fn load_config<P: AsRef<Path>>(path: P) -> Result<Config> {
    let file = std::fs::File::open(path).context("Failed to open config file")?;
    let config: Config = serde_yaml::from_reader(file).context("Failed to parse YAML config")?;
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<()> {
    for input in &config.inputs {
        if !input.path.exists() {
            return Err(anyhow::anyhow!(
                "Input path does not exist: {}",
                input.path.display()
            ));
        }

        if !config.outputs.contains_key(&input.output) {
            return Err(anyhow::anyhow!(
                "Output '{}' referenced by input '{}' does not exist",
                input.output,
                input.path.display()
            ));
        }

        if !config.presets.contains_key(&input.preset) {
            return Err(anyhow::anyhow!(
                "Preset '{}' referenced by input '{}' does not exist",
                input.preset,
                input.path.display()
            ));
        }
    }

    for (_name, output) in &config.outputs {
        if !output.path.exists() {
            std::fs::create_dir_all(&output.path).context(format!(
                "Failed to create output directory: {}",
                output.path.display()
            ))?;
        }
    }

    Ok(())
}
