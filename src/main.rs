use actix_web::HttpServer;
use filedl::{
    app_data::AppData,
    build_app,
    config::Config,
    error::{BindSnafu, MainError, ServerSnafu, ShutdownSnafu},
};
use snafu::ResultExt as _;
use std::sync::Arc;

async fn run(app_data: Arc<AppData>, host: String, port: u16) -> Result<(), MainError> {
    HttpServer::new(move || {
        let app_data = Arc::clone(&app_data);
        build_app!(app_data)
    })
    .bind((host.as_str(), port))
    .context(BindSnafu { host, port })?
    .run()
    .await
    .context(ServerSnafu)?;
    Ok(())
}

// Returning `Report` prints the error messages with their sources,
// a plain error would be printed using its `Debug` implementation.
#[actix_web::main]
async fn main() -> Result<(), snafu::Report<MainError>> {
    main_inner().await.map_err(snafu::Report::from_error)
}

async fn main_inner() -> Result<(), MainError> {
    env_logger::Builder::from_default_env()
        .filter_module("hayro_syntax", log::LevelFilter::Info)
        .init();

    let config = Config::get()?;
    let host = config.bind_address.clone();
    let port = config.bind_port;
    let app_data = AppData::with_config(config, true).await?;
    let app_data_shutdown = Arc::clone(&app_data);

    log::info!("Will bind to {}:{}", host, port);

    // Shutdown has to run even if the server failed, to write pending metadata changes.
    let run_result = run(app_data, host, port).await;
    let shutdown_result = app_data_shutdown.shutdown().await;

    match (run_result, shutdown_result) {
        (Err(run_error), Err(shutdown_error)) => Err(MainError::DoubleError {
            source: Box::new(run_error),
            shutdown_error,
        }),
        (run_result, shutdown_result) => {
            run_result?;
            shutdown_result.context(ShutdownSnafu)?;
            Ok(())
        }
    }
}
