use thiserror::Error;

#[derive(Error, Debug)]
pub enum UpdateError {
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Failed to read current version: {0}")]
    VersionReadError(#[from] std::io::Error),
    #[error("Failed to read Token: {0}")]
    TokenReadError(String),
    #[error("Invalid version format in version file: {0}")]
    VersionFormatError(#[from] std::num::ParseIntError),
    #[error("API client error: {0}")]
    ApiClientError(#[from] reqwest::Error),
    #[error("API request failed: {status} - {message}")]
    ApiRequestFailed { status: reqwest::StatusCode, message: String },
    #[error("No update available or service up-to-date")]
    NoUpdateAvailable,
    #[error("Download error: {0}")]
    DownloadError(String),
    #[error("Head error: {0}")]
    HeadError(String),
    #[error("Decryption error: {0}")]
    DecryptionError(String),
    #[error("Encryption error (internal): {0}")]
    EncryptionError(String), // Should not happen for decryption but good for aes_gcm::Error
    #[error("Archive extraction error: {0}")]
    ArchiveError(String),
    #[error("Update script execution failed: {0}")]
    ScriptError(String),
    #[error("Filesystem error: {0}")]
    FileSystemError(String),
    #[error("Hex decoding error for key: {0}")]
    HexError(#[from] hex::FromHexError),
    #[error("I/O error during file operation: {0}")]
    FileIOError(String),
    #[error("Temporary directory/file creation error: {0}")]
    TempFileError(String),
}

// Helper to convert aes_gcm::Error to UpdateError::DecryptionError
impl From<aes_gcm::Error> for UpdateError {
    fn from(err: aes_gcm::Error) -> Self {
        UpdateError::DecryptionError(err.to_string())
    }
}