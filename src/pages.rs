use crate::app_data::{AppData, DirListingItem, FiledlError, ResolvedObject};
use crate::breadcrumbs::BreadcrumbsIterator;
use actix_files::NamedFile;
use actix_web::{
    get,
    http::{header, header::DispositionType, StatusCode},
    routes, web,
    web::Redirect,
    Either, HttpResponse, Responder, ResponseError,
};
use askama::Template;
use chrono_tz::Tz;
use serde::Deserialize;
use std::path::{Path, PathBuf};
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
    Download,
    Thumb64,
    Thumb128,
    Thumb256,
}

#[derive(Debug, Deserialize)]
struct DownloadQuery {
    key: Option<String>,
    #[serde(default)]
    mode: DownloadMode,
    #[serde(default)]
    cache_hash: Option<String>,
}

/// generate a cache control header based on the cache_hash received
fn cache_control(cache_hash: Option<&str>) -> (&'static str, &'static str) {
    (
        "Cache-Control",
        if cache_hash.is_some() {
            // 1 year
            "max-age=31536000, immutable"
        } else {
            "no-cache"
        },
    )
}

/// User visible error
#[derive(Error, Debug)]
enum UserError {
    #[error("Not Found")]
    NotFound,
    #[error("Internal Server Error")]
    InternalError,
    #[error("Not implemented")]
    NotImplemented,
}

impl ResponseError for UserError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
            UserError::NotFound => StatusCode::NOT_FOUND,
            UserError::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            UserError::NotImplemented => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<FiledlError> for UserError {
    fn from(value: FiledlError) -> Self {
        match value {
            FiledlError::ObjectNotFound => Self::NotFound,
            FiledlError::Unlisted => Self::NotFound,
            FiledlError::IOError { source } => match source.kind() {
                std::io::ErrorKind::NotFound => Self::NotFound,
                _ => {
                    log::error!("Converting to user error: {}", source);
                    Self::InternalError
                }
            },
        }
    }
}

// TODO: Responsive images
#[derive(Template)]
#[template(path = "dir_listing.html")]
struct DirListingTemplate<'a> {
    // TODO: Proper URL escaping!
    app_name: &'a str,
    display_timezone: &'a Tz,
    /// List of path elements to this directory, rooted at the download directory
    download_base_url: &'a str,
    static_content_hash: &'a str,

    directory_path: &'a str,
    directory_breadcrumbs: BreadcrumbsIterator<'a>,
    items: Vec<DirListingItem>,
}

impl<'a> DirListingTemplate<'a> {
    fn new(
        app: &'a AppData,
        object_path: &'a str,
        mut items: Vec<DirListingItem>,
    ) -> DirListingTemplate<'a> {
        let mut collator = feruca::Collator::default();
        items.sort_unstable_by(|a, b| collator.collate(a.name.as_bytes(), b.name.as_bytes()));
        DirListingTemplate {
            app_name: app.get_app_name(),
            display_timezone: app.get_display_timezone(),
            download_base_url: app.get_download_base_url(),
            static_content_hash: app.get_static_content_hash(),
            directory_path: object_path,
            directory_breadcrumbs: BreadcrumbsIterator::new(object_path),
            items,
        }
    }
}

#[derive(Template, Default)]
#[template(path = "style.css", escape = "none")]
struct StylesheetTemplate {}

#[derive(Template, Default)]
#[template(path = "gallery.js", escape = "none")]
struct GalleryJsTemplate {}

#[routes]
#[get("/index.html")]
#[get("/")]
async fn index_redirect() -> impl Responder {
    Redirect::to("/download").permanent()
}

#[get("/admin")]
async fn admin(app: web::Data<Arc<AppData>>) -> impl Responder {
    "TODO"
}

#[get("/admin/thumbnail_cache_stats")]
async fn thumbnail_cache_stats(app: web::Data<Arc<AppData>>) -> HttpResponse {
    HttpResponse::Ok().json(app.get_thumbnail_cache_stats().await)
}

#[get("/download")]
async fn download_root(app: web::Data<Arc<AppData>>) -> Result<HttpResponse, UserError> {
    Ok(HttpResponse::Ok().body(
        DirListingTemplate::new(&app, "", app.list_objects().await?)
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
            "style.css" => static_content::<StylesheetTemplate>(query.cache_hash.as_deref())
                .await
                .map(Either::Right),
            "gallery.js" => static_content::<GalleryJsTemplate>(query.cache_hash.as_deref())
                .await
                .map(Either::Right),
            &_ => Err(UserError::NotFound),
        }
    } else {
        let resolved_object = app
            .resolve_object(object_path.as_str(), query.key.as_deref())
            .await?;

        match resolved_object {
            ResolvedObject::File(f) => match query.mode {
                DownloadMode::Default => file_download(&f, false).await.map(Either::Left),
                DownloadMode::Internal => unreachable!("Was handled before"),
                DownloadMode::Download => file_download(&f, true).await.map(Either::Left),
                DownloadMode::Thumb64 => thumb_download(&app, f, 64, query.cache_hash.as_deref())
                    .await
                    .map(Either::Right),
                DownloadMode::Thumb128 => thumb_download(&app, f, 128, query.cache_hash.as_deref())
                    .await
                    .map(Either::Right),
                DownloadMode::Thumb256 => thumb_download(&app, f, 256, query.cache_hash.as_deref())
                    .await
                    .map(Either::Right),
            },
            ResolvedObject::Directory(items) => match query.mode {
                DownloadMode::Default => dir_listing(&app, &object_path, items)
                    .await
                    .map(Either::Right),
                DownloadMode::Internal => unreachable!("Was handled before"),
                DownloadMode::Download => Err(UserError::NotImplemented),
                _ => Ok(Either::Right(
                    HttpResponse::BadRequest().body("Not a valid mode for directory."),
                )),
            },
        }
    }
}

async fn static_content<Tmpl: Template + Default>(
    cache_hash: Option<&str>,
) -> Result<HttpResponse, UserError> {
    Ok(HttpResponse::Ok()
        .insert_header(header::ContentType(
            Tmpl::MIME_TYPE
                .parse()
                .expect("Askama's MIME string should be valid"),
        ))
        .insert_header(cache_control(cache_hash))
        .body(
            Tmpl::default()
                .render()
                .map_err(|_| UserError::InternalError)?,
        ))
}

async fn file_download(f: &Path, force_download: bool) -> Result<NamedFile, UserError> {
    let mut nf = NamedFile::open_async(f)
        .await
        .map_err(|_| UserError::InternalError)?;

    if force_download {
        let mut cd = nf.content_disposition().clone();
        cd.disposition = DispositionType::Attachment;
        nf = nf.set_content_disposition(cd);
    }

    Ok(nf)
}

async fn thumb_download(
    app: &AppData,
    f: PathBuf,
    size: u32,
    cache_hash: Option<&str>,
) -> Result<HttpResponse, UserError> {
    let (thumb, hash) = app
        .get_thumbnail(f, (size, size))
        .await
        .map_err(|_| UserError::InternalError)?;
    Ok(HttpResponse::Ok()
        .insert_header(header::ContentType(mime::IMAGE_JPEG))
        .insert_header(header::ETag(header::EntityTag::new_strong(hash)))
        .insert_header(cache_control(cache_hash))
        .body(thumb))

    // TODO: Support HEAD request, that only verifies the cache hash, and doesn't
    // recompute the thumbnail unless necessary (if client has the image cached, but
    // is unsure about the validity, and we don't have it cached any more)
    // TODO: Proper browser caching control
}

async fn dir_listing(
    app: &AppData,
    object_path: &str,
    items: Vec<DirListingItem>,
) -> Result<HttpResponse, UserError> {
    Ok(HttpResponse::Ok().insert_header(cache_control(None)).body(
        DirListingTemplate::new(app, object_path, items)
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
        .service(thumbnail_cache_stats)
        .service(download_root)
        .service(download_object);
}

mod filters {
    use chrono::prelude::*;
    use chrono_tz::Tz;

    pub fn time_format(time: &Option<DateTime<Utc>>, tz: &Tz) -> askama::Result<String> {
        let Some(time) = time else {
            return Ok("".into());
        };

        let converted = time.with_timezone(tz);

        Ok(format!(
            "{}",
            converted.format(
                r#"<time datetime="%+">%Y-%m-%d<span class="separator">T</span>%H:%M:%S</time>"#
            )
        ))
    }
}

// TODO: Handle internal errors better, clean up all map_err. Use logging.
