use crate::app_data::{AppData, DirListingItem, ObjectResolutionError, ResolvedObject};
use crate::breadcrumbs::BreadcrumbsIterator;
use actix_files::NamedFile;
use actix_web::{
    get, http::StatusCode, routes, web, web::Redirect, Either, HttpResponse, Responder,
    ResponseError,
};
use askama::Template;
use serde::Deserialize;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Deserialize)]
struct DownloadQuery {
    key: Option<String>,
    //#[serde(default)]
    //thumbnail: bool
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
    /// List of path elements to this directory, rooted at the download directory
    download_base_url: &'a str,
    directory_path: &'a str,
    directory_breadcrumbs: BreadcrumbsIterator<'a>,
    items: Vec<DirListingItem>,
}

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
    Ok(HttpResponse::Ok().body(
        DirListingTemplate {
            download_base_url: app.get_download_base_url(),
            directory_path: "",
            directory_breadcrumbs: BreadcrumbsIterator::new(""),
            items: app.list_objects().await?,
        }
        .render()
        .unwrap(),
    ))
}

#[get("/download/{object:.*}")]
async fn download_object(
    app: web::Data<Arc<AppData>>,
    path: web::Path<String>,
    query: web::Query<DownloadQuery>,
) -> Result<Either<NamedFile, HttpResponse>, UserError> {
    let object_path = path.into_inner();
    let resolved_object = app
        .resolve_object(object_path.as_str(), query.key.as_deref())
        .await?;

    match resolved_object {
        ResolvedObject::File(f) => Ok(Either::Left(
            NamedFile::open_async(f)
                .await
                .map_err(|_| UserError::InternalError)?,
        )),
        ResolvedObject::Directory(items) => Ok(Either::Right(
            HttpResponse::Ok().body(
                DirListingTemplate {
                    download_base_url: app.get_download_base_url(),
                    directory_path: &object_path,
                    directory_breadcrumbs: BreadcrumbsIterator::new(&object_path),
                    items,
                }
                .render()
                .map_err(|_| UserError::InternalError)?,
            ),
        )),
    }
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
