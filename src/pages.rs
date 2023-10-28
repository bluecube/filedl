use crate::app_data::{AppData, DirListingItem, ObjectResolutionError, ResolvedObject};
use crate::breadcrumbs::BreadcrumbsIterator;
use actix_files::NamedFile;
use actix_web::{
    get,
    http::{header, StatusCode},
    routes, web,
    web::Redirect,
    Either, HttpResponse, Responder, ResponseError,
};
use askama::Template;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

pub const PROJECT_NAME: &str = env!("CARGO_PKG_NAME");
pub const PROJECT_REPO: &str = env!("CARGO_PKG_REPOSITORY");
pub const PROJECT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DownloadMode {
    #[default]
    Default,
    Internal,
}

#[derive(Debug, Deserialize)]
struct DownloadQuery {
    key: Option<String>,
    #[serde(default)]
    mode: DownloadMode,
}

/// User visible error
#[derive(Error, Debug)]
enum UserError {
    #[error("Not Found")]
    NotFound,
    #[error("Internal Server Error")]
    InternalError,
}

impl ResponseError for UserError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
            UserError::NotFound => StatusCode::NOT_FOUND,
            UserError::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<ObjectResolutionError> for UserError {
    fn from(value: ObjectResolutionError) -> Self {
        match value {
            ObjectResolutionError::ObjectNotFound => Self::NotFound,
            ObjectResolutionError::Unlisted => Self::NotFound,
            ObjectResolutionError::IOError { source } => match source.kind() {
                std::io::ErrorKind::NotFound => Self::NotFound,
                _ => Self::InternalError,
            },
        }
    }
}

#[derive(Template)]
#[template(path = "dir_listing.html")]
struct DirListingTemplate<'a> {
    app_name: &'a str,

    /// List of path elements to this directory, rooted at the download directory
    download_base_url: &'a str,
    directory_path: &'a str,
    directory_breadcrumbs: BreadcrumbsIterator<'a>,
    items: &'a [DirListingItem],
}

#[derive(Template)]
#[template(path = "style.css", escape = "none")]
struct StylesheetTemplate {}

#[routes]
#[get("/index.html")]
#[get("/")]
async fn index_redirect() -> impl Responder {
    Redirect::to("/download").permanent()
}

#[get("/admin")]
async fn admin(app: web::Data<AppData>) -> impl Responder {
    "TODO"
}

#[get("/download")]
async fn download_root(app: web::Data<Arc<AppData>>) -> Result<HttpResponse, UserError> {
    let objects = app.list_objects().await?;
    Ok(HttpResponse::Ok().body(
        DirListingTemplate {
            app_name: app.get_app_name(),
            download_base_url: app.get_download_base_url(),
            directory_path: "",
            directory_breadcrumbs: BreadcrumbsIterator::new(""),
            items: &objects,
        }
        .render()
        .map_err(|_| UserError::InternalError)?,
    ))
}

#[get("/download/{object:.*}")]
async fn download_object(
    app: web::Data<Arc<AppData>>,
    path: web::Path<String>,
    query: web::Query<DownloadQuery>,
) -> Result<Either<NamedFile, HttpResponse>, UserError> {
    let object_path = path.into_inner();

    if query.mode == DownloadMode::Internal {
        match object_path.as_str() {
            "style.css" => stylesheet().await.map(Either::Right),
            &_ => Err(UserError::NotFound),
        }
    } else {
        let resolved_object = app
            .resolve_object(object_path.as_str(), query.key.as_deref())
            .await?;

        match resolved_object {
            ResolvedObject::File(f) => file_download(&f).await.map(Either::Left),
            ResolvedObject::Directory(items) => dir_listing(&app, &object_path, &items)
                .await
                .map(Either::Right),
        }
    }
}

async fn stylesheet() -> Result<HttpResponse, UserError> {
    Ok(HttpResponse::Ok()
        .insert_header(header::ContentType(mime::TEXT_CSS))
        .body(
            StylesheetTemplate {}
                .render()
                .map_err(|_| UserError::InternalError)?,
        ))
}

async fn file_download(f: &Path) -> Result<NamedFile, UserError> {
    NamedFile::open_async(f)
        .await
        .map_err(|_| UserError::InternalError)
}

async fn dir_listing(
    app: &AppData,
    object_path: &str,
    items: &[DirListingItem],
) -> Result<HttpResponse, UserError> {
    Ok(HttpResponse::Ok().body(
        DirListingTemplate {
            app_name: app.get_app_name(),
            download_base_url: app.get_download_base_url(),
            directory_path: object_path,
            directory_breadcrumbs: BreadcrumbsIterator::new(object_path),
            items,
        }
        .render()
        .map_err(|_| UserError::InternalError)?,
    ))
}

/// Not found handler used for default route
async fn default_service() -> Result<HttpResponse, UserError> {
    Err(UserError::NotFound)
}

pub fn configure_pages(cfg: &mut web::ServiceConfig) {
    cfg.default_service(web::to(default_service))
        .service(index_redirect)
        .service(admin)
        .service(download_root)
        .service(download_object);
}

mod filters {
    use chrono::prelude::*;

    pub fn time_format(time: &DateTime<Utc>) -> askama::Result<String> {
        Ok(time.to_rfc3339())
    }
}
