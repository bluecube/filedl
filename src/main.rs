use actix_web::HttpServer;
use filedl::{app_data::AppData, build_app, config::Config, error::StartupError};
use std::sync::Arc;

#[actix_web::main]
async fn main() -> Result<(), StartupError> {
    env_logger::Builder::from_default_env()
        .filter_module("hayro_syntax", log::LevelFilter::Info)
        .init();

    let config = Config::get()?;
    let host = config.bind_address.clone();
    let port = config.bind_port;
    let app_data = Arc::new(AppData::with_config(config)?);

    log::info!("Will bind to {}:{}", host, port);

    HttpServer::new(move || {
        let app_data = Arc::clone(&app_data);
        build_app!(app_data)
    })
    .bind((host, port))?
    .run()
    .await?;

    Ok(())
}
