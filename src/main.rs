mod app_data;
mod config;
mod pages;
mod storage;

use crate::pages::configure_pages;

use actix_web::{middleware, web::Data, App, HttpServer};
use app_data::AppData;
use config::Config;
use std::sync::Arc;

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
            .wrap(
                middleware::DefaultHeaders::new().add(("Content-Type", "text/html; charset=utf-8")),
            )
            .configure(configure_pages)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await?;

    Ok(())
}
