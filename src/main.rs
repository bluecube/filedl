mod app_data;
mod config;
mod storage;

use actix_files::NamedFile;
use actix_web::{
    get,
    http::{header::ContentType, StatusCode},
    middleware, routes, web,
    web::{Data, Redirect},
    App, Either, HttpResponse, HttpServer, Responder, ResponseError,
};
use app_data::{AppData, DirListingItem, ObjectResolutionError, ResolvedObject};
use config::Config;
use serde::Deserialize;
use std::fmt::Write;
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

#[routes]
#[get("/index.html")]
#[get("/")]
async fn index_redirect() -> impl Responder {
    Redirect::to("/download").permanent()
}

#[get("/admin")]
async fn admin(app: web::Data<AppData>) -> impl Responder {
    let mut ret: String = "Admin".into();
    for (name, _) in &app.iter_objects().await {
        ret += "\n";
        ret += name;
    }
    ret
}

#[get("/download")]
async fn download_root(app: web::Data<Arc<AppData>>) -> impl Responder {
    let mut ret: String = "Download root".into();
    for (name, _) in app
        .iter_objects()
        .await
        .into_iter()
        .filter(|&(_, o)| o.unlisted_key.is_none())
    {
        ret += "\n";
        ret += name;
    }
    ret
}

#[get("/download/{object:.*}")]
async fn download_object(
    app: web::Data<Arc<AppData>>,
    path: web::Path<String>,
    query: web::Query<DownloadQuery>,
) -> Result<Either<NamedFile, HttpResponse>, UserError> {
    let object_path = path.into_inner();
    let (object_id, subobject_path) = match object_path.split_once('/') {
        Some((object_id, subobject_path)) => (object_id, Some(subobject_path)),
        None => (object_path.as_str(), None),
    };
    dbg!(&object_id);
    dbg!(&subobject_path);

    let resolved_object = app
        .resolve_object(object_id, subobject_path, query.key.as_deref())
        .await?;

    match resolved_object {
        ResolvedObject::File(f) => match NamedFile::open_async(f).await {
            Ok(f) => Ok(Either::Left(f)),
            Err(_) => Err(UserError::InternalError),
        },
        ResolvedObject::Directory(entries) => Ok(Either::Right(
            HttpResponse::Ok()
                .content_type(ContentType::html())
                .body(render_dir_listing(entries)),
        )),
    }
}

fn render_dir_listing(iter: impl IntoIterator<Item = DirListingItem>) -> String {
    let mut ret = "<ul>".to_string();
    for item in iter {
        write!(
            ret,
            "<li><a href=\"{}\">{}</a></li>\n",
            item.link, item.name
        );
    }
    ret += "</ul>";
    ret
}

/// Not found handler used for default route
async fn not_found() -> Result<HttpResponse, UserError> {
    Err(UserError::NotFound)
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let config = Config::get()?;
    let app_data = Arc::new(AppData::with_config(config)?);

    HttpServer::new(move || {
        let app_data = Arc::clone(&app_data);
        App::new()
            .app_data(Data::new(app_data))
            .wrap(middleware::NormalizePath::trim())
            .default_service(web::to(not_found))
            .service(index_redirect)
            .service(admin)
            .service(download_root)
            .service(download_object)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await?;

    Ok(())
}
