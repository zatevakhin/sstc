use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;

use owo_colors::OwoColorize;
mod config;
mod file_check;
mod transcoder;
use transcoder::Transcoder;
mod watcher;
use watcher::DirectoryWatcher;

const FFMPEG_BIN_NAME: &str = "ffmpeg";
const FFPROBE_BIN_NAME: &str = "ffprobe";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Config file to use
    #[arg(short, long)]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt().with_ansi(true).init();

    info!("Starting video transcoder service");

    match which::which(FFMPEG_BIN_NAME) {
        Ok(path) => info!(
            "Found {} at: {}",
            FFMPEG_BIN_NAME.green(),
            path.display().green()
        ),
        Err(e) => println!("{} not found in PATH: {}", FFMPEG_BIN_NAME.red(), e.red()),
    };

    match which::which(FFPROBE_BIN_NAME) {
        Ok(path) => info!(
            "Found {} at: {}",
            FFPROBE_BIN_NAME.green(),
            path.display().green()
        ),
        Err(e) => println!("{} not found in PATH: {}", FFPROBE_BIN_NAME.red(), e.red()),
    };

    info!("Loading configuration from {}", args.config.yellow());
    let config = config::load_config(&args.config).context("Failed to load configuration")?;

    let config = std::sync::Arc::new(config);

    let transcoder = std::sync::Arc::new(Transcoder::new(config.clone()));

    let watcher = DirectoryWatcher::new(config.clone(), transcoder);

    watcher.start_watching().await?;

    tokio::signal::ctrl_c().await?;
    info!("Received shutdown signal, shutting down...");

    Ok(())
}
