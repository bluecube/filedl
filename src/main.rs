mod app_data;
mod breadcrumbs;
mod config;
mod error;
mod pages;
mod storage;
mod thumbnails;

use crate::pages::configure_pages;

use actix_web::{http::header, middleware, web::Data, App, HttpServer};
use app_data::AppData;
use config::Config;
use error::Result;
use std::sync::Arc;

#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::init();

    let config = Config::get()?;
    let host = config.bind_address.clone();
    let port = config.bind_port;
    let app_data = Arc::new(AppData::with_config(config)?);

    log::info!("Will bind to {}:{}", host, port);

    HttpServer::new(move || {
        let app_data = Arc::clone(&app_data);
        App::new()
            .app_data(Data::new(app_data))
            .wrap(middleware::NormalizePath::trim())
            .wrap(middleware::DefaultHeaders::new().add(header::ContentType::html()))
            .configure(configure_pages)
    })
    .bind((host, port))?
    .run()
    .await?;

    Ok(())
}
