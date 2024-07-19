use crate::breadcrumbs::BreadcrumbsIterator;
use crate::error::FiledlError;
use crate::{
    app_data::{AppData, DirListingItem, ItemType, ResolvedObject},
    error::Result,
};
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
use std::sync::Arc;

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

impl ResponseError for FiledlError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
            FiledlError::ObjectNotFound => StatusCode::NOT_FOUND,
            FiledlError::Unlisted => StatusCode::NOT_FOUND,
            FiledlError::BadDownloadMode => StatusCode::NOT_FOUND,
            FiledlError::IOError { source } => match source.kind() {
                std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
                _ => {
                    log::error!("Converting to user error: {}", source);
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            },
            source => {
                log::error!("Converting to user error: {}", source);
                StatusCode::INTERNAL_SERVER_ERROR
            }
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
async fn download_root(app: web::Data<Arc<AppData>>) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok()
        .body(DirListingTemplate::new(&app, "", app.list_objects().await?).render()?))
}

#[get("/download/{object:.*}")]
async fn download_object(
    app: web::Data<Arc<AppData>>,
    path: web::Path<String>,
    query: web::Query<DownloadQuery>,
) -> Result<Either<NamedFile, HttpResponse>> {
    let object_path = path.into_inner();

    if query.mode == DownloadMode::Internal {
        match object_path.as_str() {
            "style.css" => static_content::<StylesheetTemplate>(query.cache_hash.as_deref())
                .await
                .map(Either::Right),
            "gallery.js" => static_content::<GalleryJsTemplate>(query.cache_hash.as_deref())
                .await
                .map(Either::Right),
            &_ => Err(FiledlError::ObjectNotFound),
        }
    } else {
        let resolved_object = app
            .resolve_object(object_path.as_str(), query.key.as_deref())
            .await?;

        match resolved_object.item_type() {
            ItemType::Directory => match query.mode {
                DownloadMode::Default => {
                    let items = resolved_object.list().await?;
                    dir_listing(&app, &object_path, query.key.as_deref(), items)
                        .await
                        .map(Either::Right)
                }
                DownloadMode::Download => Err(FiledlError::UnimplementedZipDownload),
                DownloadMode::Internal => unreachable!("Was handled before"),
                _ => Err(FiledlError::BadDownloadMode),
            },
            _ => match query.mode {
                DownloadMode::Default => file_download(resolved_object, false)
                    .await
                    .map(Either::Left),
                DownloadMode::Download => {
                    file_download(resolved_object, true).await.map(Either::Left)
                }
                DownloadMode::Thumb64 => {
                    thumb_download(resolved_object, 64, query.cache_hash.as_deref())
                        .await
                        .map(Either::Right)
                }
                DownloadMode::Thumb128 => {
                    thumb_download(resolved_object, 128, query.cache_hash.as_deref())
                        .await
                        .map(Either::Right)
                }
                DownloadMode::Thumb256 => {
                    thumb_download(resolved_object, 256, query.cache_hash.as_deref())
                        .await
                        .map(Either::Right)
                }
                DownloadMode::Internal => unreachable!("Was handled before"),
            },
        }
    }
}

async fn static_content<Tmpl: Template + Default>(
    cache_hash: Option<&str>,
) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok()
        .insert_header(header::ContentType(
            Tmpl::MIME_TYPE
                .parse()
                .expect("Askama's MIME string should be valid"),
        ))
        .insert_header(cache_control(cache_hash))
        .body(Tmpl::default().render()?))
}

async fn file_download<'a>(
    resolved_object: ResolvedObject<'a>,
    force_download: bool,
) -> Result<NamedFile> {
    let mut nf = NamedFile::open_async(resolved_object.path()).await?;

    if force_download {
        let mut cd = nf.content_disposition().clone();
        cd.disposition = DispositionType::Attachment;
        nf = nf.set_content_disposition(cd);
    }

    Ok(nf)
}

async fn thumb_download<'a>(
    resolved_object: ResolvedObject<'a>,
    size: u32,
    cache_hash: Option<&str>,
) -> Result<HttpResponse> {
    let (thumb, hash) = resolved_object.into_thumbnail((size, size)).await?;
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
    query_key: Option<&str>,
    items: Vec<DirListingItem>,
) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok()
        .insert_header(cache_control(None))
        .body(DirListingTemplate::new(app, object_path, items).render()?))
}

/// Not found handler used for default route
async fn default_service() -> Result<HttpResponse> {
    Err(FiledlError::ObjectNotFound)
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
