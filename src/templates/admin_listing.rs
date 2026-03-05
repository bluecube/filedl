use super::{
    AssetUrl,
    page::Page,
    util::{FormatedIsoTimestamp, ItemUrl, ThumbnailImg, url_encode},
};
use chrono_tz::Tz;
use horrorshow::{RenderOnce, TemplateBuffer, html};
use humansize::{BINARY, format_size};

use crate::app_data::{AdminObjectInfo, AppData, ItemType, ObjectOwnership};

pub struct AdminListing<'a> {
    app_name: &'a str,
    download_base_url: &'a str,
    objects_base_url: &'a str,
    display_timezone: &'a Tz,
    static_content_hash: &'a str,
    items: Vec<AdminObjectInfo>,
}

pub struct Title<'a> {
    app_name: &'a str,
}

impl RenderOnce for Title<'_> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        tmpl << format_args!("Admin — {}", self.app_name);
    }
}

impl<'a> AdminListing<'a> {
    pub fn new_wrapped(
        app: &'a AppData,
        mut items: Vec<AdminObjectInfo>,
    ) -> Page<'a, Title<'a>, AdminListing<'a>> {
        let mut collator = feruca::Collator::default();
        items.sort_unstable_by(|a, b| {
            collator.collate(a.item.name.as_bytes(), b.item.name.as_bytes())
        });

        let listing = AdminListing {
            app_name: app.get_app_name(),
            download_base_url: app.get_download_base_url(),
            objects_base_url: app.get_admin_objects_base_url(),
            display_timezone: app.get_display_timezone(),
            static_content_hash: app.get_static_content_hash(),
            items,
        };
        Page {
            download_base_url: app.get_download_base_url(),
            title: Title {
                app_name: app.get_app_name(),
            },
            content: listing,
            static_content_hash: app.get_static_content_hash(),
            display_timezone: app.get_display_timezone(),
        }
    }

    fn render_item(&self, tmpl: &mut TemplateBuffer<'_>, info: &AdminObjectInfo) {
        let item = &info.item;
        let item_url = ItemUrl {
            base_url: self.objects_base_url,
            directory_path: "",
            item_name: item.name.as_ref(),
            unlisted_key: info.unlisted_key.as_deref(),
        };
        let download_url = match &info.unlisted_key {
            Some(key) => format!(
                "{}/{}?key={}",
                self.download_base_url,
                url_encode(&item.name),
                key
            ),
            None => format!("{}/{}", self.download_base_url, url_encode(&item.name)),
        };
        let ownership_display = match &info.ownership {
            ObjectOwnership::Owned => "Owned".to_owned(),
            ObjectOwnership::Linked(path) => format!("Linked: {}", path),
        };
        let visibility_display = if info.unlisted_key.is_some() {
            "Unlisted"
        } else {
            "Public"
        };
        let thumbnail_class = if item.is_thumbnailable {
            "main-link"
        } else {
            "main-link no-thumbnail"
        };
        tmpl << html!(
            li(class = format_args!("{} admin-object", item.item_type)) {
                a(class = thumbnail_class, href = download_url.as_str()) {
                    @ if item.is_thumbnailable {
                        : ThumbnailImg(item_url);
                    }
                    span(class = "underlined") {
                        : item.name.as_ref();
                        @ if item.item_type == ItemType::Directory {
                            : "/";
                        }
                    }
                }
                div(class = "details-outer") {
                    div(class = "details-inner") {
                        span(class = "ownership"): ownership_display.as_str();
                        span(class = "visibility"): visibility_display;
                        @ if item.item_type != ItemType::Directory {
                            span(class = "size"): format_size(item.file_size, BINARY);
                        }
                        @ if let Some(modified) = item.modified {
                            : FormatedIsoTimestamp(modified.with_timezone(self.display_timezone));
                        }
                    }
                    button(
                        type = "button",
                        value = item.name.as_ref(),
                        onclick = "deleteObject(this)"
                    ): "Delete";
                }
            }
        );
    }

    fn asset_url(&self, file_name: &'a str) -> AssetUrl<'a> {
        AssetUrl {
            download_base_url: self.download_base_url,
            file_name,
            cache_hash: self.static_content_hash,
        }
    }
}

impl RenderOnce for AdminListing<'_> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        tmpl << html!(
            nav {
                @ if !self.app_name.is_empty() {
                    div(class = "app-name"): self.app_name;
                }
                h1(class = "breadcrumbs"): "Admin";
            }

            section(id = "content", class="admin") {
                @ if self.items.is_empty() {
                    div(class = "empty-dir-listing"): "No objects";
                } else {
                    ul(class = "dir-listing") {
                        @ for item in self.items.iter() {
                            |tmpl| self.render_item(tmpl, item);
                        }
                    }
                }

                form(id = "create-form") {
                    h2: "Add object";

                    div {
                        input(type = "radio", name = "mode", id="mode-upload", value = "upload", checked);
                        label(for = "mode-upload"): "Upload";
                        input(type = "file", name = "file");
                    }

                    div {
                        input(type = "radio", name = "mode", id="mode-link", value = "link");
                        label(for = "mode-link"): "Link";
                        input(
                            type = "text",
                            name = "path",
                            placeholder = "path/to/file",
                            class = "inactive"
                        );
                    }

                    div {
                        input(type = "checkbox", name = "override-id");
                        label(for = "object-id"): "ID";
                        input(type = "text", name = "object-id", id="object-id", required, readonly);
                    }

                    div {
                        input(type = "checkbox", name = "unlisted", id = "unlisted");
                        label(for = "unlisted"): "Unlisted";
                    }

                    button(type = "submit"): "Add";
                    span(class = "result");
                }

            }

            script(src = self.asset_url("admin.min.js"), defer);
        );
    }
}
