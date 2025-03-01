use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

pub async fn is_file_valid<P: AsRef<Path>>(path: P) -> Result<bool> {
    let path = path.as_ref();

    if !wait_for_stable_size(path).await? {
        return Ok(false);
    }

    // Use ffprobe to check if the file is valid
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(path)
        .output()
        .context("Failed to execute ffprobe")?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        warn!("FFprobe failed for {}: {}", path.display(), error);
        return Ok(false);
    }

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match duration_str.parse::<f64>() {
        Ok(duration) if duration > 0.0 => {
            debug!(
                "File {} is valid with duration {}s",
                path.display(),
                duration
            );
            Ok(true)
        }
        _ => {
            warn!(
                "File {} has invalid duration: {}",
                path.display(),
                duration_str
            );
            Ok(false)
        }
    }
}

async fn wait_for_stable_size<P: AsRef<Path>>(path: P) -> Result<bool> {
    let path = path.as_ref();
    let check_interval = Duration::from_secs(1);
    let timeout = Duration::from_secs(60);
    // NOTE: Mb more time, can be some buffering on copy or on write while recording.
    let stability_threshold = Duration::from_secs(3);

    let start_time = Instant::now();
    let mut last_size = None;
    let mut last_change_time = Instant::now();

    while start_time.elapsed() < timeout {
        match path.metadata() {
            Ok(metadata) => {
                let current_size = metadata.len();

                if let Some(size) = last_size {
                    if size == current_size {
                        // Size hasn't changed
                        if last_change_time.elapsed() >= stability_threshold {
                            debug!(
                                "File size stable at {} bytes for {:?}",
                                current_size, stability_threshold
                            );
                            return Ok(true);
                        }
                    } else {
                        // Size changed, reset timer
                        last_size = Some(current_size);
                        last_change_time = Instant::now();
                        debug!("File size changed to {} bytes", current_size);
                    }
                } else {
                    // First check
                    last_size = Some(current_size);
                    debug!("Initial file size: {} bytes", current_size);
                }
            }
            Err(e) => {
                warn!("Failed to get file metadata: {}", e);
                return Err(anyhow::anyhow!("Failed to get file metadata: {}", e));
            }
        }

        tokio::time::sleep(check_interval).await;
    }

    warn!("Timeout waiting for file size to stabilize");
    Ok(false)
}
