use actix_web::{HttpRequest, HttpResponse, http::StatusCode};
use horrorshow::Template as _;
use rand::Rng as _;
use snafu::prelude::*;
use std::future::Future;

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

impl FiledlError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            FiledlError::AppDataError { source, .. } => source.status_code(),
            FiledlError::BadDownloadMode { .. } => StatusCode::NOT_FOUND,
            FiledlError::IOError { source, .. } => match source.kind() {
                std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Generates an error hash and logs the full error chain.
fn log_error(req: &HttpRequest, err: &FiledlError) -> (StatusCode, String, Vec<String>) {
    let status = err.status_code();
    let hash = format!("{:08x}", rand::rng().next_u32());

    let chain: Vec<_> = snafu::ErrorCompat::iter_chain(err)
        .map(|e| e.to_string())
        .collect();
    log::error!(
        "[{}] {} {} {}:{}",
        hash,
        status,
        req.method(),
        req.uri(),
        chain
            .iter()
            .fold(String::new(), |acc, e| acc + "\n  - " + e)
    );

    (status, hash, chain)
}

/// Wraps an async handler body, rendering an HTML error page on failure.
///
/// On `Ok`, returns the response directly. On `Err`, generates a random error
/// hash, logs the full error chain with the hash, and renders a styled error page.
pub async fn styled_error_wrapper(
    req: &HttpRequest,
    app: &crate::app_data::AppData,
    fut: impl Future<Output = Result<HttpResponse>>,
) -> HttpResponse {
    let err = match fut.await {
        Ok(response) => return response,
        Err(err) => err,
    };

    let (status, hash, _chain) = log_error(req, &err);

    let is_admin = req.path().starts_with(app.get_admin_url());
    let base_url = if is_admin {
        app.get_admin_objects_base_url()
    } else {
        app.get_download_base_url()
    };

    match crate::templates::ErrorPage::new_wrapped(app, base_url, status, &hash, is_admin)
        .into_string()
    {
        Ok(html) => HttpResponse::build(status)
            .content_type(mime::TEXT_HTML_UTF_8)
            .body(html),
        Err(_) => HttpResponse::build(status)
            .content_type(mime::TEXT_PLAIN_UTF_8)
            .body(format!(
                "{} {}\nError reference: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown Error"),
                hash
            )),
    }
}

/// Wraps an async handler body, returning a JSON error response on failure.
pub async fn json_error_wrapper(
    req: &HttpRequest,
    fut: impl Future<Output = Result<HttpResponse>>,
) -> HttpResponse {
    let err = match fut.await {
        Ok(response) => return response,
        Err(err) => err,
    };

    let (status, hash, chain) = log_error(req, &err);

    HttpResponse::build(status).json(serde_json::json!({
        "error": status.canonical_reason().unwrap_or("Unknown Error"),
        "error_chain": chain,
        "error_reference": hash,
    }))
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
