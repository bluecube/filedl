pub type Result<T> = std::result::Result<T, FiledlError>;

#[derive(Debug, thiserror::Error)]
pub enum FiledlError {
    #[error("Object not found")]
    ObjectNotFound,
    #[error("Object exists, but is unlisted")]
    Unlisted,
    #[error("Attempting to use unsupported download mode")]
    BadDownloadMode,
    #[error("Zip downloads are unimplemented")]
    UnimplementedZipDownload,
    #[error("Template error: {source}")]
    TemplateError {
        #[from]
        #[source]
        source: horrorshow::Error,
    },
    #[error("Image error: {source}")]
    ImageError {
        #[from]
        #[source]
        source: image::error::ImageError,
    },
    #[error("Error when reading configuration: {source}")]
    ConfigError {
        #[from]
        #[source]
        source: figment::Error,
    },
    #[error("IO error: {source}")]
    IOError {
        #[from]
        #[source]
        source: std::io::Error,
    },
}
