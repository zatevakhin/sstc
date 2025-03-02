use crate::config::Config;
use crate::transcoder::Transcoder;
use anyhow::{Context, Result};
use notify::{EventKind, RecursiveMode, Watcher};
use owo_colors::OwoColorize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

pub struct DirectoryWatcher {
    config: Arc<Config>,
    transcoder: Arc<Transcoder>,
    _watcher: Option<Box<dyn Watcher + Send>>,
}

impl DirectoryWatcher {
    pub fn new(config: Arc<Config>, transcoder: Arc<Transcoder>) -> Self {
        Self {
            config,
            transcoder,
            _watcher: None,
        }
    }

    pub async fn start_watching(&mut self) -> Result<()> {
        let (tx, mut rx) = mpsc::channel(100);

        let mut watcher = notify::recommended_watcher(move |res| match res {
            Ok(event) => {
                if let Err(e) = tx.blocking_send(event) {
                    error!("Failed to send event: {}", e);
                }
            }
            Err(e) => error!("Watch error: {}", e),
        })?;

        for input in &self.config.inputs {
            info!("Watching directory: {}", input.path.display().green());
            watcher
                .watch(&input.path, RecursiveMode::Recursive)
                .context(format!(
                    "Failed to watch directory: {}",
                    input.path.display().green()
                ))?;

            self.process_existing_files(&input.path).await?;
        }

        let transcoder = self.transcoder.clone();

        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let Some(path) = event.paths.first() {
                    if Self::is_create_or_modify_event(&event.kind) && path.is_file() {
                        debug!("File event: {:?} at {}", event.kind, path.display());

                        let path_clone = path.to_path_buf();
                        let transcoder_clone = transcoder.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            if let Err(e) = transcoder_clone.process_file(&path_clone).await {
                                error!("Failed to process file {}: {}", path_clone.display(), e);
                            }
                        });
                    }
                }
            }
        });

        // Store the watcher in the struct so it doesn't get dropped
        self._watcher = Some(Box::new(watcher));

        info!("Directory watcher started");
        Ok(())
    }

    async fn process_existing_files(&self, dir: &Path) -> Result<()> {
        info!("Processing existing files in {}", dir.display());

        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.is_dir() {
                Box::pin(self.process_existing_files(&path)).await?;
            } else if path.is_file() {
                debug!("Found existing file: {}", path.display());
                let transcoder = self.transcoder.clone();
                let path_clone = path.clone();

                tokio::spawn(async move {
                    if let Err(e) = transcoder.process_file(&path_clone).await {
                        error!(
                            "Failed to process existing file {}: {}",
                            path_clone.display(),
                            e
                        );
                    }
                });
            }
        }

        Ok(())
    }

    fn is_create_or_modify_event(kind: &EventKind) -> bool {
        use notify::event::ModifyKind;
        matches!(
            kind,
            EventKind::Create(_)
                | EventKind::Modify(ModifyKind::Data(_))
                | EventKind::Modify(ModifyKind::Metadata(_))
        )
    }
}
