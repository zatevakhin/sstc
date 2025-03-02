use crate::config::{Config, PresetConfig};
use anyhow::Result;
use owo_colors::OwoColorize;
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

pub struct PresetGenerator;

impl PresetGenerator {
    /// Generate example presets and add them to the config
    pub fn generate_example_presets(config: &mut Config) -> Result<()> {
        info!("Generating example presets...");

        // Fast preset (low quality, quick encoding)
        let fast_h264 = PresetConfig {
            video_codec: Some("libx264".to_string()),
            audio_codec: Some("aac".to_string()),
            pixel_format: Some("yuv420p".to_string()),
            video_bitrate: Some("2M".to_string()),
            audio_bitrate: Some("128k".to_string()),
            scale: None,
            extra_options: {
                let mut options = HashMap::new();
                options.insert("-preset".to_string(), "ultrafast".to_string());
                options.insert("-crf".to_string(), "28".to_string());
                options.insert("-tune".to_string(), "fastdecode".to_string());
                options
            },
        };

        // Medium preset (balanced quality/speed)
        let medium_h264 = PresetConfig {
            video_codec: Some("libx264".to_string()),
            audio_codec: Some("aac".to_string()),
            pixel_format: Some("yuv420p".to_string()),
            video_bitrate: Some("4M".to_string()),
            audio_bitrate: Some("192k".to_string()),
            scale: None,
            extra_options: {
                let mut options = HashMap::new();
                options.insert("-preset".to_string(), "medium".to_string());
                options.insert("-crf".to_string(), "23".to_string());
                options.insert("-tune".to_string(), "film".to_string());
                options
            },
        };

        // Slow preset (high quality, slower encoding)
        let slow_h264 = PresetConfig {
            video_codec: Some("libx264".to_string()),
            audio_codec: Some("aac".to_string()),
            pixel_format: Some("yuv420p".to_string()),
            video_bitrate: Some("6M".to_string()),
            audio_bitrate: Some("256k".to_string()),
            scale: None,
            extra_options: {
                let mut options = HashMap::new();
                options.insert("-preset".to_string(), "slow".to_string());
                options.insert("-crf".to_string(), "18".to_string());
                options.insert("-tune".to_string(), "film".to_string());
                options.insert("-x264-params".to_string(), "ref=5:me=umh".to_string());
                options
            },
        };

        // Fast H.265/HEVC preset
        let fast_h265 = PresetConfig {
            video_codec: Some("libx265".to_string()),
            audio_codec: Some("aac".to_string()),
            pixel_format: Some("yuv420p10le".to_string()),
            video_bitrate: None,
            audio_bitrate: Some("128k".to_string()),
            scale: None,
            extra_options: {
                let mut options = HashMap::new();
                options.insert("-preset".to_string(), "ultrafast".to_string());
                options.insert("-crf".to_string(), "28".to_string());
                options.insert("-tag:v".to_string(), "hvc1".to_string());
                options
            },
        };

        // Medium H.265/HEVC preset
        let medium_h265 = PresetConfig {
            video_codec: Some("libx265".to_string()),
            audio_codec: Some("aac".to_string()),
            pixel_format: Some("yuv420p10le".to_string()),
            video_bitrate: None,
            audio_bitrate: Some("192k".to_string()),
            scale: None,
            extra_options: {
                let mut options = HashMap::new();
                options.insert("-preset".to_string(), "medium".to_string());
                options.insert("-crf".to_string(), "23".to_string());
                options.insert("-tag:v".to_string(), "hvc1".to_string());
                options.insert("-x265-params".to_string(), "log-level=error".to_string());
                options
            },
        };

        // Slow/High Quality H.265/HEVC preset
        let slow_h265 = PresetConfig {
            video_codec: Some("libx265".to_string()),
            audio_codec: Some("aac".to_string()),
            pixel_format: Some("yuv420p10le".to_string()),
            video_bitrate: None,
            audio_bitrate: Some("256k".to_string()),
            scale: None,
            extra_options: {
                let mut options = HashMap::new();
                options.insert("-preset".to_string(), "slow".to_string());
                options.insert("-crf".to_string(), "18".to_string());
                options.insert("-tag:v".to_string(), "hvc1".to_string());
                options.insert(
                    "-x265-params".to_string(),
                    "ref=5:me=star:rd=4:log-level=error".to_string(),
                );
                options
            },
        };

        // Create special GoPro preset that reduces size while maintaining quality
        let gopro_compact = PresetConfig {
            video_codec: Some("libx265".to_string()),
            audio_codec: Some("copy".to_string()),
            pixel_format: Some("yuv420p10le".to_string()),
            video_bitrate: None,
            audio_bitrate: None,
            scale: None,
            extra_options: {
                let mut options = HashMap::new();
                options.insert("-preset".to_string(), "fast".to_string());
                options.insert("-crf".to_string(), "24".to_string());
                options.insert("-x265-params".to_string(), "log-level=error".to_string());
                options.insert("-tag:v".to_string(), "hvc1".to_string());
                options.insert(
                    "-map".to_string(),
                    "0:v,0:a,0:m:handler_name:GoPro MET".to_string(),
                );
                options.insert("-map_metadata".to_string(), "0".to_string());
                options.insert("-movflags".to_string(), "use_metadata_tags".to_string());
                options
            },
        };

        // Insert presets into config if they don't already exist
        let presets_to_add = [
            ("fast_h264", fast_h264),
            ("medium_h264", medium_h264),
            ("slow_h264", slow_h264),
            ("fast_h265", fast_h265),
            ("medium_h265", medium_h265),
            ("slow_h265", slow_h265),
            ("gopro_compact", gopro_compact),
        ];

        for (name, preset) in presets_to_add {
            if !config.presets.contains_key(name) {
                config.presets.insert(name.to_string(), preset);
                info!("Added preset: {}", name.green());
            } else {
                info!("Preset {} already exists, skipping", name.yellow());
            }
        }

        Ok(())
    }

    /// Save the config with example presets to a file
    pub fn save_example_presets<P: AsRef<Path>>(path: P) -> Result<()> {
        let mut config = Config {
            inputs: Vec::new(),
            outputs: HashMap::new(),
            presets: HashMap::new(),
            max_parallel_jobs: Some(1),
        };

        Self::generate_example_presets(&mut config)?;

        let yaml = serde_yaml::to_string(&config)?;
        std::fs::write(&path, yaml)?;

        info!(
            "Saved example presets to {}",
            path.as_ref().display().green()
        );
        Ok(())
    }
}
