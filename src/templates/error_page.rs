use super::page::Page;
use crate::app_data::{AppData, InterfaceContext};
use actix_web::http::StatusCode;
use horrorshow::{RenderOnce, html, prelude::TemplateBuffer};

pub struct ErrorPage<'a> {
    app_name: &'a str,
    status_code: StatusCode,
    error_hash: &'a str,
    user_message: Option<String>,
    is_admin: bool,
}

impl<'a> ErrorPage<'a> {
    pub fn new_wrapped(
        app: &'a AppData,
        context: InterfaceContext,
        status_code: StatusCode,
        error_hash: &'a str,
        user_message: Option<String>,
    ) -> Page<'a, String, ErrorPage<'a>> {
        let reason = status_code.canonical_reason().unwrap_or("Unknown Error");
        let title = format!("{} {}", status_code.as_u16(), reason);

        Page {
            asset_base_url: context.get_objects_base_url(app),
            static_content_hash: app.get_static_content_hash(),
            display_timezone: app.get_display_timezone(),
            is_admin: context.is_admin(),
            has_scripts: false,

            title,
            content: ErrorPage {
                app_name: app.get_app_name(),
                status_code,
                error_hash,
                user_message,
                is_admin: context.is_admin(),
            },
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
                h1: &status_line;
            }

            section(id = "content", class? = self.is_admin.then_some("admin")) {
                @ if let Some(ref msg) = self.user_message {
                    p: msg;
                }
                p {
                    : "Error reference: ";
                    code(id = "error-reference"): self.error_hash;
                }
            }
        );
    }
}
