use std::sync::Arc;

pub type Result<T> = std::result::Result<T, FiledlError>;

#[derive(Debug, thiserror::Error)]
pub enum FiledlError {
    #[error("Object not found")]
    ObjectNotFound,
    #[error("Object {object_id} already exists")]
    ObjectExists { object_id: Arc<str> },
    #[error("Unlisted object {path} accessed with wrong key {key:?}")]
    Unlisted { path: String, key: Option<String> },
    #[error("Attempting to use unsupported download mode")]
    BadDownloadMode,
    #[error("Directory traversal in path {path}")]
    DirectoryTraversal { path: String },
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
    #[error("Error when extracting request payload: {source}")]
    PayloadError {
        #[from]
        #[source]
        source: actix_web::error::PayloadError,
    },
}
