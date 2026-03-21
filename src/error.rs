use snafu::prelude::*;

pub type Result<T> = std::result::Result<T, FiledlError>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum FiledlError {
    #[snafu(display("App error at {location}"))]
    #[snafu(context(false))]
    AppDataError {
        source: crate::app_data::AppDataError,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Attempting to use unsupported download mode at {location}"))]
    BadDownloadMode {
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Template rendering failed at {location}"))]
    TemplateError {
        source: horrorshow::Error,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("Zip archive creation failed at {location}"))]
    ZippityBuildError {
        #[snafu(source(from(zippity::AddDirectoryRecursiveError, Box::new)))]
        source: Box<zippity::AddDirectoryRecursiveError>,
        #[snafu(implicit)]
        location: snafu::Location,
    },

    #[snafu(display("IO error at {location}"))]
    IOError {
        source: std::io::Error,
        #[snafu(implicit)]
        location: snafu::Location,
    },
}

#[derive(Debug, Snafu)]
pub enum StartupError {
    #[snafu(transparent)]
    ConfigError {
        #[snafu(source(from(figment::Error, Box::new)))]
        source: Box<figment::Error>,
    },

    #[snafu(transparent)]
    IOError { source: std::io::Error },
}
