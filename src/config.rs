use crate::error::UpdateError;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub service_name: String,
    pub current_version_file: PathBuf,
    pub update_check_api_url: String,
    pub status_report_api_url: String,
    pub poll_interval_seconds: u64,
    pub download_base_dir: PathBuf,
    pub decryption_key_hex: String,
    pub update_script_name: String,
    pub device_token: String,
}

impl Config {
    pub fn load(path: &str) -> Result<Self, UpdateError> {
        let config_str = fs::read_to_string(path).map_err(|e| {
            UpdateError::ConfigError(format!("Failed to read config file '{}': {}", path, e))
        })?;
        let config: Config = toml::from_str(&config_str)
            .map_err(|e| UpdateError::ConfigError(format!("Failed to parse TOML config: {}", e)))?;

        // Validate decryption key length (64 hex chars for 32 bytes)
        if config.decryption_key_hex.len() != 64 {
            return Err(UpdateError::ConfigError(
                "Decryption key hex string must be 64 characters long for a 32-byte key."
                    .to_string(),
            ));
        }
        // Ensure download_base_dir exists
        if !config.download_base_dir.exists() {
            fs::create_dir_all(&config.download_base_dir).map_err(|e| {
                UpdateError::FileSystemError(format!(
                    "Failed to create download base directory {:?}: {}",
                    config.download_base_dir, e
                ))
            })?;
        }

        Ok(config)
    }

    pub fn get_decryption_key(&self) -> Result<Vec<u8>, UpdateError> {
        hex::decode(&self.decryption_key_hex).map_err(UpdateError::from)
    }
}

pub fn get_current_version(config: &Config) -> Result<i32, UpdateError> {
    if !config.current_version_file.exists() {
        tracing::warn!(
            "Version file {:?} not found, assuming version 0.",
            config.current_version_file
        );
        return Ok(0); // Default to 0 if file doesn't exist
    }
    let version_str = fs::read_to_string(&config.current_version_file)?;
    Ok(version_str.trim().parse()?)
}
