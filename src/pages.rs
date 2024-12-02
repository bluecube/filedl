use crate::{
    app_data::{AppData, DirListingItem, ItemType, ResolvedObject},
    error::{FiledlError, Result},
    templates,
    thumbnails::ThumbnailType,
};
use actix_files::NamedFile;
use actix_web::{
    body::{BoxBody, EitherBody},
    get,
    http::{header, StatusCode},
    put, routes,
    web::{self, Payload, Redirect},
    CustomizeResponder, HttpRequest, HttpResponse, Responder, ResponseError,
};
use horrorshow::Template as _;
use memchr::memmem;
use serde::{Deserialize, Serialize};
use std::{panic, sync::Arc};
use tokio::task::spawn_blocking;
use walkdir::WalkDir;

pub const PROJECT_NAME: &str = env!("CARGO_PKG_NAME");
pub const PROJECT_REPO: &str = env!("CARGO_PKG_REPOSITORY");
pub const PROJECT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DownloadMode {
    #[default]
    Default,
    Assets,
    Download,
    Thumbnail,
}

#[derive(Debug, Deserialize)]
struct DownloadQuery {
    key: Option<String>,
    #[serde(default)]
    mode: DownloadMode,
    #[serde(default)]
    size: u32,
    #[serde(default)]
    thumbnail_type: ThumbnailType,
    #[serde(default)]
    cache_hash: Option<String>,
}

#[derive(Debug)]
enum ContentEncoding {
    Identity,
    Brotli,
}

#[derive(Debug, Serialize)]
struct UploadResult {
    download_url: String,
}

const CACHE_CONTROL_IMMUTABLE: (&str, &str) = (
    "Cache-Control",
    "max-age=31536000, immutable", // 1 year
);

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
        // TODO: Clean this up!
        match self {
            FiledlError::ObjectNotFound => StatusCode::NOT_FOUND,
            FiledlError::Unlisted { path: _, key: _ } => StatusCode::NOT_FOUND,
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

#[routes]
#[get("/index.html")]
#[get("/")]
async fn index_redirect(app: web::Data<Arc<AppData>>) -> impl Responder {
    Redirect::to(app.get_download_base_url().to_owned()).permanent()
}

#[get("/admin")]
async fn admin(app: web::Data<Arc<AppData>>) -> impl Responder {
    "TODO"
}

#[put("/admin/objects/{object:.*}")]
async fn rest_file_upload(
    app: web::Data<Arc<AppData>>,
    path: web::Path<String>,
    payload: Payload,
) -> Result<HttpResponse> {
    let object_path = path.into_inner();
    app.upload_simple_object(object_path.as_str().into(), payload)
        .await?;

    let download_url = format!("{}/{}", app.get_download_base_url(), &object_path);
    Ok(HttpResponse::Ok().json(UploadResult { download_url }))
}

#[get("/admin/thumbnail_cache_stats")]
async fn thumbnail_cache_stats(app: web::Data<Arc<AppData>>) -> HttpResponse {
    HttpResponse::Ok().json(app.get_thumbnail_cache_stats().await)
}

fn select_thumbnail_type(req: &HttpRequest) -> ThumbnailType {
    if req
        .headers()
        .get(header::ACCEPT)
        .is_some_and(|value| memmem::find(value.as_bytes(), b"image/avif").is_some())
    {
        ThumbnailType::Avif
    } else {
        ThumbnailType::Jpeg
    }
}

fn select_content_encoding(req: &HttpRequest) -> ContentEncoding {
    if req
        .headers()
        .get(header::ACCEPT_ENCODING)
        .is_some_and(|value| memmem::find(value.as_bytes(), b"br").is_some())
    {
        ContentEncoding::Brotli
    } else {
        ContentEncoding::Identity
    }
}

#[get("/download")]
async fn download_root(app: web::Data<Arc<AppData>>, req: HttpRequest) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().content_type(mime::TEXT_HTML_UTF_8).body(
        templates::DirListing::new_wrapped(
            &app,
            "",
            None,
            select_thumbnail_type(&req),
            app.list_objects().await?,
        )
        .into_string()?,
    ))
}

#[get("/download/{object:.*}")]
async fn download_object(
    app: web::Data<Arc<AppData>>,
    path: web::Path<String>,
    query: web::Query<DownloadQuery>,
    req: HttpRequest,
) -> Result<HttpResponse> {
    let object_path = path.into_inner();
    Ok(if query.mode == DownloadMode::Assets {
        asset_download(&app, &object_path, &req)?
    } else {
        let resolved_object = app
            .resolve_object(object_path.as_str(), query.key.as_deref())
            .await?;

        match resolved_object.item_type() {
            ItemType::Directory => match query.mode {
                DownloadMode::Default => {
                    let items = resolved_object.list().await?;
                    dir_listing(
                        &app,
                        &object_path,
                        query.key.as_deref(),
                        select_thumbnail_type(&req),
                        items,
                    )
                    .await?
                }
                DownloadMode::Download => zip_download(&req, resolved_object).await?,
                DownloadMode::Assets => unreachable!("Was handled before"),
                _ => return Err(FiledlError::BadDownloadMode),
            },
            _ => match query.mode {
                DownloadMode::Default => file_download(resolved_object, false, &req).await?,
                DownloadMode::Download => file_download(resolved_object, true, &req).await?,
                DownloadMode::Thumbnail => {
                    thumb_download(
                        resolved_object,
                        query.size,
                        query.cache_hash.as_deref(),
                        query.thumbnail_type,
                    )
                    .await?
                }
                _ => unreachable!("Was handled before"),
            },
        }
    })
}

async fn file_download(
    resolved_object: ResolvedObject<'_>,
    force_download: bool,
    req: &HttpRequest,
) -> Result<HttpResponse> {
    let mut nf = NamedFile::open_async(resolved_object.path()).await?;

    if force_download {
        let mut cd = nf.content_disposition().clone();
        cd.disposition = header::DispositionType::Attachment;
        nf = nf.set_content_disposition(cd);
    }

    Ok(nf.respond_to(req))
}

async fn thumb_download<'a>(
    resolved_object: ResolvedObject<'a>,
    size: u32,
    cache_hash: Option<&str>,
    thumbnail_type: ThumbnailType,
) -> Result<HttpResponse> {
    let size = match size {
        n if n <= 64 => 64,
        n if n <= 128 => 128,
        _ => 256,
    };

    let (thumb, hash) = resolved_object
        .into_thumbnail((size, size), thumbnail_type)
        .await?;
    Ok(HttpResponse::Ok()
        .insert_header(header::ContentType(thumbnail_type.mime()))
        .insert_header(header::ETag(header::EntityTag::new_strong(hash)))
        .insert_header(cache_control(cache_hash))
        .body(thumb))

    // TODO: Support HEAD request, that only verifies the cache hash, and doesn't
    // recompute the thumbnail unless necessary (if client has the image cached, but
    // is unsure about the validity, and we don't have it cached any more)
    // TODO: Proper browser caching control
}

fn asset_download(app: &AppData, object_path: &str, req: &HttpRequest) -> Result<HttpResponse> {
    // icons.css is generated on the fly, so we handle it separately
    if object_path == "icons.css" {
        return Ok(HttpResponse::Ok()
            .insert_header(header::ContentType(mime::TEXT_CSS))
            .insert_header(CACHE_CONTROL_IMMUTABLE)
            .body(templates::icons_css(
                app.get_download_base_url(),
                app.get_static_content_hash(),
            )));
    }

    let (content, brotli_content, ct) = assets(object_path).ok_or(FiledlError::ObjectNotFound)?;

    let mut response_builder = HttpResponse::Ok();
    response_builder
        .insert_header(header::ContentType(ct))
        .insert_header(CACHE_CONTROL_IMMUTABLE);

    Ok(match select_content_encoding(req) {
        ContentEncoding::Identity => response_builder.body(content),
        ContentEncoding::Brotli => response_builder
            .insert_header(header::ContentEncoding::Brotli)
            .body(brotli_content),
    })
}

async fn dir_listing(
    app: &AppData,
    object_path: &str,
    query_key: Option<&str>,
    thumbnail_type: ThumbnailType,
    items: Vec<DirListingItem>,
) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok()
        .content_type(mime::TEXT_HTML_UTF_8)
        .insert_header(cache_control(None))
        .body(
            templates::DirListing::new_wrapped(app, object_path, query_key, thumbnail_type, items)
                .into_string()?,
        ))
}

/// Returns the content of resolved_object zipped using zippity.
/// Because of the hairy zippity response type, we directly convert to the response and don't bother
/// returning the Responder.
async fn zip_download<'a>(
    req: &HttpRequest,
    resolved_object: ResolvedObject<'a>,
) -> Result<HttpResponse> {
    let walkdir = WalkDir::new(resolved_object.path());
    let dir_path = resolved_object.path().to_owned();
    let zip_file_name = format!(
        "{}.zip",
        dir_path
            .file_name()
            .expect("Zip downloads should only work for directories with normal name")
            .to_string_lossy()
    );

    let builder_join_handle = spawn_blocking(
        move || -> Result<zippity::Builder<zippity::TokioFileEntry>> {
            let mut builder = zippity::Builder::new();
            let dir_name = dir_path
                .file_name()
                .expect("Zip downloads should only work for directories with normal name")
                .to_string_lossy();

            for entry in walkdir {
                let entry = entry.map_err(std::io::Error::from)?;

                if !entry.file_type().is_file() {
                    continue;
                }

                let metadata = entry.metadata().map_err(std::io::Error::from)?;
                let path = entry.into_path();
                let entry_name = format!(
                    "{}/{}",
                    &dir_name,
                    path.strip_prefix(&dir_path)
                        .expect("The prefix is always taken from the path. (Symlinks!!!!?)")
                        .display()
                );
                builder.add_entry_with_size(entry_name, path, metadata.len())?;
            }

            Ok(builder)
        },
    );

    let builder = match builder_join_handle.await {
        Ok(builder) => builder?,
        Err(e) => {
            panic::resume_unwind(
                e.try_into_panic()
                    .expect("The builder task should never be cancelled"),
            );
        }
    };

    let responder = builder.build().into_responder();

    let response = neutralize_customize_responder(
        responder
            .customize()
            .append_header(header::ContentDisposition::attachment(zip_file_name)),
        req,
    );
    Ok(response)
}

fn neutralize_customize_responder<T>(r: CustomizeResponder<T>, req: &HttpRequest) -> HttpResponse
where
    T: Responder<Body = BoxBody>,
{
    r.respond_to(req).map_body(|_, body| match body {
        EitherBody::Left { body } => body,
        EitherBody::Right { body } => body,
    })
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
        .service(rest_file_upload)
        .service(download_root)
        .service(download_object);
}

include! {concat!(env!("OUT_DIR"), "/assets/assets.rs")}
