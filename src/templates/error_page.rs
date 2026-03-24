use super::page::Page;
use crate::app_data::AppData;
use actix_web::http::StatusCode;
use horrorshow::{RenderOnce, html, prelude::TemplateBuffer};

pub struct ErrorPage<'a> {
    app_name: &'a str,
    status_code: StatusCode,
    error_hash: &'a str,
    is_admin: bool,
}

impl<'a> ErrorPage<'a> {
    pub fn new_wrapped(
        app: &'a AppData,
        base_url: &'a str,
        status_code: StatusCode,
        error_hash: &'a str,
        is_admin: bool,
    ) -> Page<'a, String, ErrorPage<'a>> {
        let reason = status_code.canonical_reason().unwrap_or("Unknown Error");
        let title = format!("{} {}", status_code.as_u16(), reason);

        Page {
            asset_base_url: base_url,
            is_admin,
            has_scripts: false,
            title,
            content: ErrorPage {
                app_name: app.get_app_name(),
                status_code,
                error_hash,
                is_admin,
            },
            static_content_hash: app.get_static_content_hash(),
            display_timezone: app.get_display_timezone(),
        }
    }
}

impl RenderOnce for ErrorPage<'_> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        let reason = self
            .status_code
            .canonical_reason()
            .unwrap_or("Unknown Error");
        let status_line = format!("{} {}", self.status_code.as_u16(), reason);

        tmpl << html!(
            nav {
                @ if !self.app_name.is_empty() {
                    div(class = "app-name"): self.app_name;
                }
            }

            section(id = "content", class? = self.is_admin.then_some("admin")) {
                h1: &status_line;
                p(class = "error-reference") {
                    : "Error reference: ";
                    code: self.error_hash;
                }
            }
        );
    }
}
