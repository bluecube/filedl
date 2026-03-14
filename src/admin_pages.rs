use crate::{
    app_data::AppData,
    error::Result,
    templates::{AdminListing, util::url_encode},
};
use actix_web::{
    HttpResponse, delete, get, patch, put,
    web::{self, Path, Payload},
};
use chrono::{DateTime, Utc};
use horrorshow::Template as _;
use relative_path::RelativePathBuf;
use std::sync::Arc;

#[derive(Debug, serde::Deserialize)]
struct AdminCreateQuery {
    link: Option<String>,
    unlisted_key: Option<Arc<str>>,
    expires: Option<DateTime<Utc>>,
}

/// Admin dashboard — lists all objects including unlisted ones
#[get("")]
async fn admin_dashboard(app: web::Data<Arc<AppData>>) -> Result<HttpResponse> {
    let items = app.list_objects_admin().await?;
    Ok(HttpResponse::Ok()
        .content_type(mime::TEXT_HTML_UTF_8)
        .body(AdminListing::new_wrapped(&app, items).into_string()?))
}

/// Create object: upload file (no ?link) or register linked path (?link=<path>)
/// Mounted under /objects/ — full path is PUT /admin/objects/{object}
#[put("/{object:.*}")]
async fn rest_create_object(
    app: web::Data<Arc<AppData>>,
    path: Path<String>,
    query: web::Query<AdminCreateQuery>,
    payload: Payload,
) -> Result<HttpResponse> {
    let object_id: Arc<str> = path.into_inner().into();
    let query = query.into_inner();
    let unlisted_key = query.unlisted_key;
    let expires = query.expires;

    let download_url = match &unlisted_key {
        Some(key) => format!(
            "{}/{}?key={}",
            app.get_download_base_url(),
            url_encode(&object_id),
            key
        ),
        None => format!("{}/{}", app.get_download_base_url(), url_encode(&object_id)),
    };

    if let Some(link_path_str) = &query.link {
        let link_path = RelativePathBuf::from(link_path_str.as_str());
        app.create_linked_object(object_id, link_path, unlisted_key, expires)
            .await?;
    } else {
        app.upload_simple_object(object_id, payload, unlisted_key, expires)
            .await?;
    }

    #[derive(serde::Serialize)]
    struct CreateResult {
        download_url: String,
    }

    Ok(HttpResponse::Ok().json(CreateResult { download_url }))
}

/// Update object metadata (visibility, expiry). Both fields are always updated.
/// `{"unlisted_key": null}` makes the object public; `{"unlisted_key": "abc"}` sets the key.
/// `{"expires": null}` clears expiry; `{"expires": "2026-12-31T00:00:00Z"}` sets it.
#[derive(Debug, serde::Deserialize)]
struct PatchObjectBody {
    unlisted_key: Option<Arc<str>>,
    expires: Option<DateTime<Utc>>,
}

#[patch("/{object:.*}")]
async fn rest_patch_object(
    app: web::Data<Arc<AppData>>,
    path: Path<String>,
    body: web::Json<PatchObjectBody>,
) -> Result<HttpResponse> {
    let object_id = path.into_inner();
    let body = body.into_inner();
    let mut obj = app.get_object_mut(&object_id).await?;
    obj.unlisted_key = body.unlisted_key;
    obj.expires = body.expires;
    Ok(HttpResponse::Ok().finish())
}

/// Delete an object (owned: removes data from disk; linked: removes metadata only)
/// Mounted under /objects/ — full path is DELETE /admin/objects/{object}
#[delete("/{object:.*}")]
async fn rest_delete_object(
    app: web::Data<Arc<AppData>>,
    path: Path<String>,
) -> Result<HttpResponse> {
    let object_id = path.into_inner();
    app.delete_object(&object_id).await?;
    Ok(HttpResponse::Ok().finish())
}

/// Thumbnail cache statistics endpoint
#[get("/thumbnail_cache_stats")]
async fn thumbnail_cache_stats(app: web::Data<Arc<AppData>>) -> HttpResponse {
    HttpResponse::Ok().json(app.get_thumbnail_cache_stats().await)
}

/// Configure all admin routes
pub fn configure_admin_pages(cfg: &mut web::ServiceConfig) {
    cfg.service(admin_dashboard)
        .service(
            web::scope("/objects")
                .configure(crate::pages::configure_pages)
                .service(rest_create_object)
                .service(rest_patch_object)
                .service(rest_delete_object),
        )
        .service(thumbnail_cache_stats);
}
