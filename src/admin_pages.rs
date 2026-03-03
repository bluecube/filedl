use crate::{app_data::AppData, error::Result};
use actix_web::{
    HttpResponse, get, put,
    web::{self, Path, Payload},
};
use std::sync::Arc;

/// File upload endpoint
#[put("/objects/{object:.*}")]
async fn rest_file_upload(
    app: web::Data<Arc<AppData>>,
    path: Path<String>,
    payload: Payload,
) -> Result<HttpResponse> {
    let object_path = path.into_inner();
    app.upload_simple_object(object_path.as_str().into(), payload)
        .await?;

    let download_url = format!("{}/{}", app.get_download_base_url(), &object_path);

    #[derive(serde::Serialize)]
    struct UploadResult {
        download_url: String,
    }

    Ok(HttpResponse::Ok().json(UploadResult { download_url }))
}

/// Thumbnail cache statistics endpoint
#[get("/thumbnail_cache_stats")]
async fn thumbnail_cache_stats(app: web::Data<Arc<AppData>>) -> HttpResponse {
    HttpResponse::Ok().json(app.get_thumbnail_cache_stats().await)
}

/// Configure all admin routes
pub fn configure_admin_pages(cfg: &mut web::ServiceConfig) {
    cfg.service(rest_file_upload).service(thumbnail_cache_stats);
}
