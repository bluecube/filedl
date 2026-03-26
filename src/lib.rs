pub mod admin_pages;
pub mod app_data;
pub mod config;
pub mod error;
pub mod pages;
pub mod storage;
pub mod templates;
pub mod thumbnails;

#[doc(hidden)]
pub fn configure_services(cfg: &mut actix_web::web::ServiceConfig) {
    cfg.service(pages::index_page)
        .service(actix_web::web::scope("/download").configure(pages::configure_pages))
        .service(actix_web::web::scope("/admin").configure(admin_pages::configure_admin_pages));
}

/// Builds a fully configured actix-web App.
/// Used by both the binary and integration tests to ensure identical app setup.
#[macro_export]
macro_rules! build_app {
    ($app_data:expr) => {
        ::actix_web::App::new()
            .app_data(::actix_web::web::Data::new($app_data))
            .wrap(::actix_web::middleware::NormalizePath::trim())
            .wrap(
                ::actix_web::middleware::DefaultHeaders::new()
                    .add(::actix_web::http::header::ContentType::html())
                    .add(("X-Content-Type-Options", "nosniff"))
                    .add(("Referrer-Policy", "no-referrer"))
                    .add(("X-Frame-Options", "SAMEORIGIN")),
            )
            .wrap(::actix_web::middleware::Compress::default())
            .configure($crate::configure_services)
            .default_service(::actix_web::web::to($crate::pages::default_service))
    };
}
