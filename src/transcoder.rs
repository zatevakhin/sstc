use crate::config::{Config, InputConfig, OutputConfig, PresetConfig};
use crate::file_check;
use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, Semaphore};
use tracing::{debug, error, info, warn};

use crate::ffprobe;

pub struct Transcoder {
    config: Arc<Config>,
    active_jobs: DashMap<PathBuf, ()>,
    job_semaphore: Arc<Semaphore>,
    file_queue: Arc<Mutex<VecDeque<PathBuf>>>,
    queue_tx: mpsc::Sender<()>,
    queue_rx: Arc<Mutex<mpsc::Receiver<()>>>,
}

#[derive(Debug, Default, Clone)]
struct FFmpegProgress {
    frame: Option<i64>,
    fps: Option<f64>,
    stream_0_0_q: Option<f64>,
    bitrate: Option<String>,
    total_size: Option<i64>,
    out_time_us: Option<i64>,
    out_time_ms: Option<i64>,
    out_time: Option<String>,
    dup_frames: Option<i64>,
    drop_frames: Option<i64>,
    speed: Option<String>,
    progress: Option<String>,
}

impl FFmpegProgress {
    fn from_key_values(key_values: &HashMap<String, String>) -> Self {
        let mut progress = Self::default();

        for (key, value) in key_values {
            match key.as_str() {
                "frame" => progress.frame = value.parse().ok(),
                "fps" => progress.fps = value.parse().ok(),
                "stream_0_0_q" => progress.stream_0_0_q = value.parse().ok(),
                "bitrate" => progress.bitrate = Some(value.clone()),
                "total_size" => progress.total_size = value.parse().ok(),
                "out_time_us" => progress.out_time_us = value.parse().ok(),
                "out_time_ms" => progress.out_time_ms = value.parse().ok(),
                "out_time" => progress.out_time = Some(value.clone()),
                "dup_frames" => progress.dup_frames = value.parse().ok(),
                "drop_frames" => progress.drop_frames = value.parse().ok(),
                "speed" => progress.speed = Some(value.clone()),
                "progress" => progress.progress = Some(value.clone()),
                _ => {} // Ignore unknown fields
            }
        }

        progress
    }

    fn is_complete(&self) -> bool {
        matches!(self.progress.as_deref(), Some("end"))
    }
}

impl Transcoder {
    pub fn new(config: Arc<Config>) -> Self {
        let max_jobs = config.max_parallel_jobs.unwrap_or(1);
        let (queue_tx, queue_rx) = mpsc::channel(100);

        info!(
            "Transcoder initialized with {} max parallel jobs",
            max_jobs.magenta()
        );

        let transcoder = Self {
            config,
            active_jobs: DashMap::new(),
            job_semaphore: Arc::new(Semaphore::new(max_jobs)),
            file_queue: Arc::new(Mutex::new(VecDeque::new())),
            queue_tx,
            queue_rx: Arc::new(Mutex::new(queue_rx)),
        };

        transcoder.start_queue_processor();

        transcoder
    }

    fn start_queue_processor(&self) {
        let queue_rx = self.queue_rx.clone();
        let this = self.clone();

        tokio::spawn(async move {
            let mut rx = queue_rx.lock().await;

            loop {
                if rx.recv().await.is_none() {
                    break;
                }

                this.process_queued_files().await;
            }
        });
    }

    async fn process_queued_files(&self) {
        loop {
            let file_path = {
                let mut queue = self.file_queue.lock().await;
                if queue.is_empty() {
                    return;
                }
                queue.pop_front()
            };

            if let Some(file_path) = file_path {
                if self.active_jobs.contains_key(&file_path) {
                    info!("Already processing file: {}", file_path.display());
                    continue;
                }

                self.spawn_file_processor(file_path).await;
            }
        }
    }

    async fn spawn_file_processor(&self, file_path: PathBuf) {
        self.active_jobs.insert(file_path.clone(), ());

        let this = self.clone();

        tokio::spawn(async move {
            let permit = match this.job_semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(e) => {
                    error!("Failed to acquire semaphore: {}", e);
                    this.active_jobs.remove(&file_path);
                    this.requeue_file(file_path).await;
                    return;
                }
            };

            let output_path_result = this.get_output_path_for_file(&file_path);

            match this.process_file_internal(&file_path).await {
                Ok(_) => {
                    info!(
                        "Successfully processed file: {}",
                        file_path.display().green()
                    );
                }
                Err(e) => {
                    error!(
                        "Error processing file {}: {}",
                        file_path.display().yellow(),
                        e.red()
                    );

                    if let Ok(output_path) = output_path_result {
                        if output_path.exists() {
                            match std::fs::remove_file(&output_path) {
                                Ok(_) => info!(
                                    "Removed incomplete output file: {}",
                                    output_path.display()
                                ),
                                Err(err) => error!(
                                    "Failed to remove incomplete output file {}: {}",
                                    output_path.display(),
                                    err
                                ),
                            }
                        }
                    }

                    if e.to_string().contains("not valid or still being copied") {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        this.requeue_file(file_path.clone()).await;
                    }
                }
            }

            this.active_jobs.remove(&file_path);

            drop(permit);
        });
    }

    async fn requeue_file(&self, file_path: PathBuf) {
        let mut queue = self.file_queue.lock().await;
        queue.push_back(file_path.clone());
        drop(queue);

        if let Err(e) = self.queue_tx.send(()).await {
            error!("Failed to signal queue processor: {}", e);
        } else {
            info!(
                "Requeued file for later processing: {}",
                file_path.display()
            );
        }
    }

    pub async fn process_file(&self, file_path: &Path) -> Result<()> {
        let Some(_) = self.find_matching_input(file_path) else {
            debug!(
                "No matching input configuration found for: {}",
                file_path.display()
            );
            return Ok(());
        };

        if !self.active_jobs.contains_key(file_path) {
            let mut queue = self.file_queue.lock().await;

            let already_queued = queue.iter().any(|p| p == file_path);
            if !already_queued {
                debug!("Adding file to queue: {}", file_path.display());
                queue.push_back(file_path.to_path_buf());

                drop(queue);
                self.queue_tx
                    .send(())
                    .await
                    .context("Failed to signal queue processor")?;
                info!(
                    "File queued for processing: {}",
                    file_path.display().green()
                );
            } else {
                debug!("File already in queue: {}", file_path.display());
            }
        } else {
            info!("File already being processed: {}", file_path.display());
        }

        Ok(())
    }

    async fn process_file_internal(&self, file_path: &Path) -> Result<()> {
        if !file_check::is_file_valid(file_path).await? {
            return Err(anyhow::anyhow!(
                "File is not valid or still being copied: {}",
                file_path.display()
            ));
        }

        let Some(input_config) = self.find_matching_input(file_path) else {
            return Err(anyhow::anyhow!("No matching input configuration found"));
        };

        let preset = self.get_preset(&input_config.preset)?;
        let output = self.get_output(&input_config.output)?;

        let output_path = self.create_output_path(file_path, &output)?;

        if output_path.exists() {
            info!(
                "Output file already exists, skipping: {}",
                output_path.display()
            );
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
                error!(
                    "Failed to transcode {}: {}",
                    file_path.display().yellow(),
                    e.red()
                );
                if output_path.exists() {
                    if let Err(e) = std::fs::remove_file(&output_path) {
                        error!("Failed to remove incomplete output file: {}", e);
                    }
                }
                return Err(e);
            }
        }

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
        let ff = ffprobe::get_format_info(input_path);

        let mut cmd = Command::new("ffmpeg");

        cmd.arg("-v").arg("quiet");
        cmd.arg("-progress").arg("pipe:1");
        cmd.arg("-stats_period").arg("1.0");

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

        info!(
            "Executing: {} {}",
            cmd.get_program().to_str().unwrap().green(),
            cmd.get_args()
                .collect::<Vec<_>>()
                .iter()
                .map(|i| { String::from_utf8_lossy(i.as_encoded_bytes()) })
                .collect::<Vec<_>>()
                .join(" ")
                .yellow()
        );

        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or(anyhow!("Failed to open stdout"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or(anyhow!("Failed to open stderr"))?;

        let stderr_reader = BufReader::new(stderr);
        tokio::spawn(async move {
            for line in stderr_reader.lines() {
                if let Ok(line) = line {
                    if !line.trim().is_empty() {
                        error!("FFmpeg error: {}", line);
                    }
                }
            }
        });

        let reader = BufReader::new(stdout);
        let mut current_progress = HashMap::new();

        let bar = match ff {
            Ok(format_info) => ProgressBar::new(format_info.duration as u64)
                .with_style(
                    ProgressStyle::with_template(
                        "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
                    )
                    .unwrap(),
                )
                .with_message("video duration"),
            Err(e) => {
                warn!("Could not get duration for {}: {}", input_path.display(), e);
                ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template(
                            "[{elapsed_precise}] {spinner} Processing... {msg}",
                        )
                        .unwrap(),
                    )
                    .with_message("transcoding")
            }
        };

        for line in reader.lines() {
            let line = line?;
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                current_progress.insert(key.to_string(), value.to_string());

                if key == "progress" {
                    let progress = FFmpegProgress::from_key_values(&current_progress);

                    if let Some(ms) = progress.out_time_ms {
                        let progress_t = (ms / 1_000_000) as u64;
                        bar.set_position(progress_t);
                    }

                    if progress.is_complete() {
                        bar.finish();
                        break;
                    }

                    current_progress.clear();
                }
            }
        }

        let status = child.wait()?;
        if !status.success() {
            return Err(anyhow!("FFmpeg process failed with status: {}", status));
        }

        if !output_path.exists() {
            return Err(anyhow!(
                "Output file was not created: {}",
                output_path.display()
            ));
        }

        let metadata = std::fs::metadata(output_path)?;
        if metadata.len() == 0 {
            return Err(anyhow!("Output file is empty: {}", output_path.display()));
        }

        Ok(())
    }

    fn get_output_path_for_file(&self, file_path: &Path) -> Result<PathBuf> {
        let Some(input_config) = self.find_matching_input(file_path) else {
            return Err(anyhow!("No matching input configuration found"));
        };

        let output = self.get_output(&input_config.output)?;
        self.create_output_path(file_path, &output)
    }

    pub fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            active_jobs: self.active_jobs.clone(),
            job_semaphore: self.job_semaphore.clone(),
            file_queue: self.file_queue.clone(),
            queue_tx: self.queue_tx.clone(),
            queue_rx: self.queue_rx.clone(),
        }
    }
}
