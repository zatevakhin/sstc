use crate::config::{Config, InputConfig, OutputConfig, PresetConfig};
use crate::file_check;
use anyhow::{Context, Result};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tracing::{error, info, warn};

pub struct Transcoder {
    config: Arc<Config>,
    active_jobs: DashMap<PathBuf, ()>,
}

impl Transcoder {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            active_jobs: DashMap::new(),
        }
    }

    pub async fn process_file(&self, file_path: &Path) -> Result<()> {
        let Some(input_config) = self.find_matching_input(file_path) else {
            return Ok(());
        };

        if self.active_jobs.contains_key(file_path) {
            info!("Already processing file: {}", file_path.display());
            return Ok(());
        }

        self.active_jobs.insert(file_path.to_path_buf(), ());

        if !file_check::is_file_valid(file_path).await? {
            warn!(
                "File is not valid or still being copied: {}",
                file_path.display()
            );
            self.active_jobs.remove(file_path);
            return Ok(());
        }

        let preset = self.get_preset(&input_config.preset)?;
        let output = self.get_output(&input_config.output)?;

        let output_path = self.create_output_path(file_path, &output)?;

        if output_path.exists() {
            info!(
                "Output file already exists, skipping: {}",
                output_path.display()
            );
            self.active_jobs.remove(file_path);
            return Ok(());
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create output directory")?;
        }

        match self.transcode_file(file_path, &output_path, &preset).await {
            Ok(_) => {
                info!(
                    "Successfully transcoded: {} -> {}",
                    file_path.display(),
                    output_path.display()
                );
            }
            Err(e) => {
                error!("Failed to transcode {}: {}", file_path.display(), e);
                if output_path.exists() {
                    if let Err(e) = std::fs::remove_file(&output_path) {
                        error!("Failed to remove incomplete output file: {}", e);
                    }
                }
            }
        }

        self.active_jobs.remove(file_path);
        Ok(())
    }

    fn find_matching_input(&self, file_path: &Path) -> Option<InputConfig> {
        let extension = file_path.extension()?.to_str()?.to_lowercase();

        for input in &self.config.inputs {
            if !file_path.starts_with(&input.path) {
                continue;
            }

            if input
                .extensions
                .iter()
                .any(|ext| ext.to_lowercase() == extension)
            {
                return Some(input.clone());
            }
        }

        None
    }

    fn get_preset(&self, preset_name: &str) -> Result<PresetConfig> {
        self.config
            .presets
            .get(preset_name)
            .cloned()
            .context(format!("Preset not found: {}", preset_name))
    }

    fn get_output(&self, output_name: &str) -> Result<OutputConfig> {
        self.config
            .outputs
            .get(output_name)
            .cloned()
            .context(format!("Output not found: {}", output_name))
    }

    fn create_output_path(
        &self,
        input_path: &Path,
        output_config: &OutputConfig,
    ) -> Result<PathBuf> {
        let filename = input_path
            .file_stem()
            .context("Failed to get file stem")?
            .to_str()
            .context("Failed to convert file stem to string")?;

        let output_filename = output_config
            .filename_template
            .replace("{filename}", filename);
        let output_path = output_config
            .path
            .join(format!("{}.{}", output_filename, output_config.container));

        Ok(output_path)
    }

    async fn transcode_file(
        &self,
        input_path: &Path,
        output_path: &Path,
        preset: &PresetConfig,
    ) -> Result<()> {
        let mut cmd = Command::new("ffmpeg");

        cmd.arg("-i").arg(input_path);
        cmd.arg("-c:v").arg(&preset.video_codec);
        cmd.arg("-c:a").arg(&preset.audio_codec);
        cmd.arg("-b:v").arg(&preset.video_bitrate);
        cmd.arg("-b:a").arg(&preset.audio_bitrate);
        cmd.arg("-vf").arg(format!("scale={}", preset.scale));

        for (key, value) in &preset.extra_options {
            cmd.arg(key).arg(value);
        }

        cmd.arg("-y").arg(output_path);

        let cmd_str = format!("{:?}", cmd);
        info!("Executing: {}", cmd_str);

        let output = cmd.output().context("Failed to execute ffmpeg")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("FFmpeg failed: {}", stderr));
        }

        Ok(())
    }
}
