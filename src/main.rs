mod api_client;
mod config;
mod error;
use api_client::ApiClient;
use config::{get_current_version, Config};
use error::UpdateError;
use std::{
    env, fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
use tokio::time::Duration;

fn unzip_update(p: &Path, o: &Path) -> Result<(), UpdateError> {
    let f = fs::File::open(p)
        .map_err(|e| UpdateError::FileSystemError(format!("Failed to open zipped files: {}", e)))?;

    let mut archive = zip::ZipArchive::new(f)
        .map_err(|e| UpdateError::ArchiveError(format!("Failed to extract zipped files: {}", e)))?;

    tracing::debug!("archive len {}", archive.len());

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| {
            UpdateError::ArchiveError(format!("Failed to extract zipped files: {}", e))
        })?;
        let out_path = match file.enclosed_name() {
            Some(path) => {
                let mut p = PathBuf::from(o);
                p.push(path);
                p
            }
            None => continue,
        };

        if file.is_dir() {
            fs::create_dir_all(&out_path).unwrap();
        } else {
            if let Some(p) = out_path.parent() {
                if !p.exists() {
                    fs::create_dir_all(p).unwrap();
                }
            }
            let mut out_file = fs::File::create(&out_path).unwrap();
            io::copy(&mut file, &mut out_file).unwrap();
        }

        // Get and Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if let Some(mode) = file.unix_mode() {
                fs::set_permissions(&out_path, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
    }

    tracing::debug!("unzipping done");

    Ok(())
}

pub fn run_update_script(
    cfg: &Config,
    script_path: &Path,
    working_dir: &Path, // The script should run from within its extracted directory
) -> Result<(), UpdateError> {
    tracing::info!(
        "Running update script {:?} in working directory {:?}",
        script_path,
        working_dir
    );

    if !script_path.exists() {
        return Err(UpdateError::ScriptError(format!(
            "Update script not found at {:?}",
            script_path
        )));
    }

    // Make script executable (e.g., chmod +x) - specific to Unix-like systems
    let metadata = std::fs::metadata(script_path).map_err(|e| {
        UpdateError::FileSystemError(format!(
            "Failed to get metadata for script {:?}: {}",
            script_path, e
        ))
    })?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o755); // Add execute permissions for user, group, others (ugo+x)
    std::fs::set_permissions(script_path, permissions).map_err(|e| {
        UpdateError::FileSystemError(format!(
            "Failed to set executable permission on script {:?}: {}",
            script_path, e
        ))
    })?;

    tracing::info!("Set executable permission on {:?}", script_path);

    let output = Command::new(script_path)
        .env("DB_PASSWORD", &cfg.db_password)
        .current_dir(working_dir) // Run the script from its own directory
        .output()
        .map_err(|e| {
            UpdateError::ScriptError(format!(
                "Failed to execute update script {:?}: {}",
                script_path, e
            ))
        })?;

    if output.status.success() {
        tracing::info!(
            "Update script executed successfully. STDOUT:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        if !output.stderr.is_empty() {
            tracing::warn!(
                "Update script STDERR (though successful):\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    } else {
        let error_message = format!(
            "Update script failed with status: {:?}.\nSTDOUT:\n{}\nSTDERR:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        tracing::error!("{}", error_message);
        Err(UpdateError::ScriptError(error_message))
    }
}

async fn run_update_cycle(
    cfg: &mut Config,
    api: &ApiClient,
    current_version: i32,
) -> Result<(), UpdateError> {
    //TODO: handle error in finding current version

    match api.check_for_updates().await {
        Ok(update_info) => {
            tracing::info!(
                "New version available: {}, URL: {}\nCurrent version: {}",
                update_info.version_code,
                update_info.file_url,
                current_version
            );
            if update_info.version_code > current_version {
                let file_name = update_info.file_url.split('/').last().unwrap();
                let mut download_path = PathBuf::from(&cfg.download_base_dir);
                download_path.push(format!("{}.zip", file_name));

                match api
                    .download_update(&update_info.file_url, &download_path)
                    .await
                {
                    Ok(_) => {
                        api.report_status(
                            current_version,
                            format!(
                                "version {} downloaded successfully",
                                update_info.version_code
                            ),
                        )
                        .await
                        .ok();

                        tracing::debug!("file is downloaded successfully");
                        let mut out_extracted_path = PathBuf::from(&cfg.download_base_dir);
                        out_extracted_path.push(file_name);
                        if let Err(e) = unzip_update(&download_path, &out_extracted_path) {
                            match &e {
                                UpdateError::ArchiveError(m) => {
                                    tracing::error!("error in unzipping file: {}", m);
                                    fs::remove_file(&download_path)?;
                                    fs::remove_dir_all(&out_extracted_path)?;
                                }
                                _ => {
                                    tracing::error!("unknown error in extracting files ");
                                }
                            }
                        } else {
                            tracing::debug!("file is extracted successfully");
                            api.report_status(
                                current_version,
                                format!(
                                    "file {} is extracted successfully",
                                    update_info.version_code
                                ),
                            )
                            .await
                            .ok();

                            let script_path = out_extracted_path.join(&cfg.update_script_name);
                            if let Err(UpdateError::ScriptError(e)) =
                                run_update_script(&cfg, &script_path, &out_extracted_path)
                            {
                                api.report_status(
                                    current_version,
                                    format!("update {} failed: {}", update_info.version_code, e),
                                )
                                .await
                                .ok();
                            } else {
                                api.report_status(
                                    current_version,
                                    format!(
                                        "updated successfully from {} to {}",
                                        current_version, update_info.version_code
                                    ),
                                )
                                .await
                                .ok();
                            }
                        }
                        cfg.poll_interval_seconds = 300;
                    }
                    Err(e) => {
                        match &e {
                            UpdateError::TimeoutError => {
                                cfg.poll_interval_seconds = 1;
                            }
                            _ => {
                                cfg.poll_interval_seconds = 300;
                            }
                        }
                        tracing::error!("error in downloading file: {}", e);
                    }
                }
            } else {
                tracing::info!("No new update available or service is up-to-date.");
            }
        }
        Err(e) => {
            tracing::warn!("update error: {}", e);
        }
    }

    Ok(())
}

fn reset_ntp_service() -> Result<(), UpdateError> {
    let _ = Command::new("/usr/bin/sudo")
        .args(["/usr/bin/systemctl", "restart", "ntp"])
        .output()
        .map_err(|e| UpdateError::ScriptError(format!("Failed to restart ntp service: {}", e)))?;
    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("embedded_updater=info".parse().unwrap()),
        )
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .init();

    tracing::info!("Embedded Updater starting...");
    let config_path =
        env::var("PODBOX_UPDATE_CONF").unwrap_or("/etc/podbox_update/config.toml".to_string()); // Or get from command line arguments
    let mut config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load configuration: {}", e);
            return;
        }
    };
    tracing::info!("Configuration loaded: {:?}", config.service_name);

    let token = config.device_token.clone();

    let api_client = ApiClient::new(config.clone(), token);

    loop {
        if let Err(e) = reset_ntp_service() {
            tracing::warn!("ntp reset error: {}", e);
        }

        let current_version = get_current_version(&config).unwrap_or(0);
        tracing::info!("Current service version: {}", current_version);

        tracing::info!("Starting update check cycle...");
        if let Err(e) = run_update_cycle(&mut config, &api_client, current_version).await {
            tracing::error!("Update cycle ended with error: {}", e);
            // Decide on error recovery strategy here. For now, we just log and continue.
        }

        tracing::info!(
            "Update check cycle finished. Sleeping for {} seconds.",
            config.poll_interval_seconds
        );
        tokio::time::sleep(Duration::from_secs(config.poll_interval_seconds)).await;
    }
}
