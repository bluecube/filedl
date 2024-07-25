use super::util::url_encode;
use chrono_tz::Tz;
use horrorshow::{helper::doctype, html, prelude::TemplateBuffer, RenderOnce};

/// Wrapper around a template that provides the header and footer.
pub struct Page<'a, T, C> {
    pub download_base_url: &'a str,
    pub static_content_hash: &'a str,
    pub display_timezone: &'a Tz,

    pub title: T,
    pub content: C,
}

impl<'a, T: RenderOnce, C: RenderOnce> RenderOnce for Page<'a, T, C> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        tmpl << html!(
            : doctype::HTML;
            html {
                head {
                    meta(
                        name = "viewport", content="width=device-width, initial-scale=1"
                    );
                    title: self.title;
                    link(
                        rel = "stylesheet",
                        href = format_args!("{}/style.css?mode=internal&cache_hash={}", url_encode(self.download_base_url), self.static_content_hash)
                    );
                    script(
                        src = format_args!("{}/gallery.js?mode=internal&cache_hash={}", url_encode(self.download_base_url), self.static_content_hash),
                        defer
                    );
                }
                body {
                    : self.content;

                    footer {
                        div {
                            a(href = crate::pages::PROJECT_REPO) {
                                : crate::pages::PROJECT_NAME;
                            }
                            : " ";
                            : crate::pages::PROJECT_VERSION;
                        }
                        div: "No cookies, no tracking, no nothing.";
                        div {
                            : "Times are in timezone ";
                            : self.display_timezone.name();
                        }
                    }
                }
            }
        );
    }
}
