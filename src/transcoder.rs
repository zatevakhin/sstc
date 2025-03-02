use crate::config::{Config, InputConfig, OutputConfig, PresetConfig};
use crate::file_check;
use anyhow::{Context, Result};
use dashmap::DashMap;
use owo_colors::OwoColorize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

pub struct Transcoder {
    config: Arc<Config>,
    active_jobs: DashMap<PathBuf, ()>,
    job_semaphore: Arc<Semaphore>,
}

impl Transcoder {
    pub fn new(config: Arc<Config>) -> Self {
        let max_jobs = config.max_parallel_jobs.unwrap_or(1);

        info!(
            "Transcoder initialized with {} max parallel jobs",
            max_jobs.magenta()
        );

        Self {
            config,
            active_jobs: DashMap::new(),
            job_semaphore: Arc::new(Semaphore::new(max_jobs)),
        }
    }

    pub async fn process_file(&self, file_path: &Path) -> Result<()> {
        let Some(input_config) = self.find_matching_input(file_path) else {
            debug!("No matching input: {}", file_path.display());
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

        // Acquire a semaphore permit before starting the transcoding
        let permit = self.job_semaphore.clone().acquire_owned().await?;

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
        drop(permit); // releasing a slot in the semaphore

        self.active_jobs.remove(file_path);
        Ok(())
    }

    fn find_matching_input(&self, file_path: &Path) -> Option<InputConfig> {
        let extension = file_path.extension()?.to_str()?.to_lowercase();

        let canonical_file_path = match std::fs::canonicalize(file_path) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to canonicalize path {}: {}", file_path.display(), e);
                return None;
            }
        };

        debug!("Checking file: {}", canonical_file_path.display());

        for input in &self.config.inputs {
            let canonical_input_path = match std::fs::canonicalize(&input.path) {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        "Failed to canonicalize input path {}: {}",
                        input.path.display(),
                        e
                    );
                    continue;
                }
            };

            debug!(
                "Comparing with input path: {}",
                canonical_input_path.display()
            );

            if !canonical_file_path.starts_with(&canonical_input_path) {
                debug!("Path doesn't match input directory");
                continue;
            }

            if input
                .extensions
                .iter()
                .any(|ext| ext.to_lowercase() == extension)
            {
                debug!("Found matching input for file: {}", file_path.display());
                return Some(input.clone());
            }
        }

        debug!("No matching input found for: {}", file_path.display());
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
        cmd.arg("-y").arg(output_path);

        if let Some(video_codec) = &preset.video_codec {
            cmd.arg("-c:v").arg(video_codec);
        }
        if let Some(audio_codec) = &preset.audio_codec {
            cmd.arg("-c:a").arg(audio_codec);
        }

        if let Some(video_bitrate) = &preset.video_bitrate {
            cmd.arg("-b:v").arg(video_bitrate);
        }
        if let Some(audio_bitrate) = &preset.audio_bitrate {
            cmd.arg("-b:a").arg(audio_bitrate);
        }

        if let Some(pixel_format) = &preset.pixel_format {
            cmd.arg("-pix_fmt").arg(pixel_format);
        }

        if let Some(scale) = &preset.scale {
            cmd.arg("-vf").arg(format!("scale={}", scale));
        }

        for (key, value) in &preset.extra_options {
            cmd.arg(key).arg(value);
        }

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
