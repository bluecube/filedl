mod admin_pages;
mod app_data;
mod config;
mod error;
mod pages;
mod storage;
mod templates;
mod thumbnails;

use crate::error::StartupError;

use actix_web::{App, HttpServer, http::header, middleware, web};
use app_data::AppData;
use config::Config;
use std::sync::Arc;

#[actix_web::main]
async fn main() -> Result<(), StartupError> {
    env_logger::init();

    let config = Config::get()?;
    let host = config.bind_address.clone();
    let port = config.bind_port;
    let app_data = Arc::new(AppData::with_config(config)?);

    log::info!("Will bind to {}:{}", host, port);

    HttpServer::new(move || {
        let app_data = Arc::clone(&app_data);
        App::new()
            .app_data(web::Data::new(app_data))
            .wrap(middleware::NormalizePath::trim())
            .wrap(middleware::DefaultHeaders::new().add(header::ContentType::html()))
            .wrap(middleware::Compress::default())
            .service(web::scope("/download").configure(pages::configure_pages))
            .service(web::scope("/admin").configure(admin_pages::configure_admin_pages))
            .service(pages::index_redirect)
            .default_service(web::to(pages::default_service))
    })
    .bind((host, port))?
    .run()
    .await?;

    Ok(())
}
