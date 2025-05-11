use crate::config::Config;
use crate::error::UpdateError;
use reqwest::{
    header::{ACCEPT_RANGES, RANGE},
    Client, ClientBuilder,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use tokio::{fs::OpenOptions, io::AsyncWriteExt};

#[derive(Deserialize, Debug, Clone)]
pub struct UpdateInfo {
    #[serde(rename = "versionCode")]
    pub version_code: i32,
    #[serde(rename = "fileUrl")]
    pub file_url: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct UpdateErr {
    pub message: String,
}

#[derive(Serialize, Debug)]
struct StatusReportPayload {
    #[serde(rename = "versionCode")]
    version_code: i32,
    #[serde(rename = "statusMessage")]
    status_message: String,
}

pub struct ApiClient {
    client: Client,
    config: Config,
    token: String,
}

impl ApiClient {
    pub fn new(config: Config, token: String) -> Self {
        ApiClient {
            client: ClientBuilder::new()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            config,
            token,
        }
    }

    pub async fn check_for_updates(&self) -> Result<UpdateInfo, UpdateError> {
        tracing::info!(
            "Checking for updates at: {}",
            self.config.update_check_api_url
        );

        let response = self
            .client
            .get(&self.config.update_check_api_url)
            .header("device-token", &self.token)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_message = response.json::<UpdateErr>().await?;
            tracing::error!(
                "Update check API request failed with status {}: {}",
                status,
                error_message.message
            );
            return Err(UpdateError::ApiRequestFailed {
                status,
                message: error_message.message,
            });
        }

        let update_info = response.json::<UpdateInfo>().await?;
        tracing::debug!("Received update info: {:?}", update_info);
        Ok(update_info)
    }

    pub async fn download_update(
        &self,
        url: &str,
        destination_path: &Path,
    ) -> Result<(), UpdateError> {
        // Ensure parent directory exists
        if let Some(parent_dir) = destination_path.parent() {
            if !parent_dir.exists() {
                std::fs::create_dir_all(parent_dir).map_err(|e| {
                    UpdateError::FileSystemError(format!(
                        "Failed to create parent directory for download: {}",
                        e
                    ))
                })?;
            }
        }

        // Step 1: Head Request
        let response = self.client.head(url).send().await?;

        if !response.status().is_success() {
            return Err(UpdateError::HeadError(format!(
                "Head request failed with status: {}",
                response.status()
            )));
        }

        let total_size_opt = response
            .headers()
            .get("x-content-length")
            .and_then(|val| val.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let supports_range = response.headers().get(ACCEPT_RANGES).map_or(false, |val| {
            val.to_str().map_or(false, |s| s.contains("bytes"))
        });

        tracing::debug!(
            "file size and range support is: {} , {}",
            total_size_opt.unwrap(),
            supports_range
        );

        // STEP 2: Determine current downloaded size

        let current_offset = if destination_path.exists() {
            tokio::fs::metadata(destination_path)
                .await
                .map_err(|e| {
                    UpdateError::FileSystemError(format!(
                        "Failed to get metadata for existing file {}: {}",
                        destination_path.display(),
                        e
                    ))
                })?
                .len()
        } else {
            0
        };
        tracing::debug!(
            "downloaded size for file {} is {}",
            destination_path.display(),
            current_offset
        );

        // Step 3: Compare downloaded size
        if let Some(total_size) = total_size_opt {
            if current_offset >= total_size && total_size > 0 {
                // total_size > 0 check for empty files
                tracing::debug!(
                    "File {} already fully downloaded ({} bytes).",
                    destination_path.display(),
                    current_offset
                );
                return Ok(());
            }
        }

        tracing::info!("Downloading from {} to {:?}", url, destination_path);

        let mut request_builder = self.client.get(url);

        if current_offset > 0 {
            request_builder = request_builder.header(RANGE, format!("bytes={}-", current_offset));
        }

        let response = request_builder.send().await?;

        if !response.status().is_success() {
            return Err(UpdateError::DownloadError(format!(
                "Download request failed with status: {}",
                response.status()
            )));
        }

        let mut dest_file_builder = OpenOptions::new();
        dest_file_builder.create(true);

        if response.status() == reqwest::StatusCode::OK {
            //NOTE: server wants to send the file from the beginning.
            dest_file_builder.write(true).truncate(true);
        } else {
            dest_file_builder.append(true);
        }
        let mut dest_file = dest_file_builder
            .open(destination_path)
            .await
            .map_err(|e| {
                UpdateError::FileIOError(format!(
                    "Failed to create destination file {:?}: {}",
                    destination_path, e
                ))
            })?;

        tracing::debug!("{:?}", response.headers());
        let mut stream = response.bytes_stream();
        while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
            let chunk = item.map_err(|e| {
                if e.to_string() == "error decoding response body" {
                    UpdateError::TimeoutError
                } else {
                    UpdateError::DownloadError(format!("Error reading download stream: {}", e))
                }
            })?;
            dest_file.write_all(&chunk).await.map_err(|e| {
                UpdateError::FileIOError(format!("Failed to write chunk to file: {}", e))
            })?;
        }

        tracing::info!("Download complete: {:?}", destination_path);
        Ok(())
    }

    pub async fn report_status(
        &self,
        version_code: i32, // The version involved in the update attempt
        status_message: String,
    ) -> Result<(), UpdateError> {
        let payload = StatusReportPayload {
            version_code,
            status_message,
        };

        tracing::info!(
            "Reporting status: {:?} to {}",
            payload,
            self.config.status_report_api_url
        );

        let response = self
            .client
            .put(&self.config.status_report_api_url)
            .header("device-token", &self.token)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            tracing::error!(
                "Status report API request failed with status {}: {}",
                status,
                error_message
            );
            return Err(UpdateError::ApiRequestFailed {
                status,
                message: error_message,
            });
        }
        tracing::info!("Status report successful");
        Ok(())
    }
}
