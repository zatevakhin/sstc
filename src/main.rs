use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::info;

use owo_colors::OwoColorize;
mod config;
mod file_check;
mod transcoder;
use transcoder::Transcoder;
mod watcher;
use watcher::DirectoryWatcher;
mod presets;
use presets::PresetGenerator;

const FFMPEG_BIN_NAME: &str = "ffmpeg";
const FFPROBE_BIN_NAME: &str = "ffprobe";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the transcoding service
    Run {
        /// Config file to use
        #[arg(short, long)]
        config: String,

        /// Override max parallel jobs
        #[arg(short = 'j', long)]
        max_jobs: Option<usize>,
    },
    /// Configuration management commands
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Generate a complete example configuration file
    Generate {
        /// Output file path
        #[arg(short, long)]
        output: String,
    },
    /// Preset management commands
    Presets {
        #[command(subcommand)]
        action: PresetsCommand,
    },
}

#[derive(Subcommand, Debug)]
enum PresetsCommand {
    /// Generate example presets and save to a file
    Generate {
        /// Output file path
        #[arg(short, long)]
        output: String,
    },
    /// Add example presets to an existing config file
    Add {
        /// Config file to modify
        #[arg(short, long)]
        config: String,
    },
    /// Show example presets in terminal
    Show,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let log_level = match args.log_level.to_lowercase().as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => {
            eprintln!("Invalid log level: {}, defaulting to INFO", args.log_level);
            tracing::Level::INFO
        }
    };

    tracing_subscriber::fmt()
        .with_ansi(true)
        .with_max_level(log_level)
        .init();

    info!("Log level is set to: {}", log_level.yellow());

    match &args.command {
        Commands::Run { config, max_jobs } => {
            run_transcoder(config, max_jobs).await?;
        }
        Commands::Config { action } => match action {
            ConfigCommand::Generate { output } => {
                info!(
                    "Generating complete example configuration to {}",
                    output.yellow()
                );
                PresetGenerator::save_example_config(output)?;
                info!("Done! You can use this file as a starting point for your configuration.");
                info!("Run with: sstc run -c {}", output.green());
            }
            ConfigCommand::Presets { action } => match action {
                PresetsCommand::Generate { output } => {
                    info!("Generating example presets to {}", output.yellow());
                    PresetGenerator::save_example_presets(output)?;
                    info!("Done! You can use this file as reference or starting point.");
                }
                PresetsCommand::Add { config } => {
                    info!("Adding example presets to config file {}", config.yellow());
                    let mut config_data = config::load_config(config)?;
                    PresetGenerator::generate_example_presets(&mut config_data)?;

                    let yaml = serde_yaml::to_string(&config_data)?;
                    std::fs::write(config, yaml)?;
                    info!("Updated config file with example presets");
                }
                PresetsCommand::Show => {
                    info!("Showing example presets:");
                    let mut empty_config = config::Config {
                        inputs: Vec::new(),
                        outputs: std::collections::HashMap::new(),
                        presets: std::collections::HashMap::new(),
                        max_parallel_jobs: Some(1),
                    };

                    PresetGenerator::generate_example_presets(&mut empty_config)?;

                    let presets_yaml = serde_yaml::to_string(&empty_config.presets)?;
                    println!("\n{}\n", presets_yaml);
                }
            },
        },
    }

    Ok(())
}
async fn run_transcoder(config_path: &str, max_jobs: &Option<usize>) -> Result<()> {
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

    info!("Loading configuration from {}", config_path.yellow());
    let mut config = config::load_config(config_path).context("Failed to load configuration")?;

    if let Some(jobs) = max_jobs {
        config.max_parallel_jobs = Some(*jobs);
    }

    let config = std::sync::Arc::new(config);
    let transcoder = std::sync::Arc::new(Transcoder::new(config.clone()));
    let mut watcher = DirectoryWatcher::new(config.clone(), transcoder);

    watcher.start_watching().await?;

    tokio::signal::ctrl_c().await?;
    info!("Received shutdown signal, shutting down...");

    Ok(())
}
