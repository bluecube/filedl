use super::{
    AssetUrl,
    page::Page,
    util::{Ellipsis, FormatedIsoTimestamp, ItemUrl, ThumbnailImg},
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
            asset_base_url: app.get_admin_objects_base_url(),
            is_admin: true,
            has_scripts: true,
            title: Title {
                app_name: app.get_app_name(),
            },
            content: listing,
            static_content_hash: app.get_static_content_hash(),
            display_timezone: app.get_display_timezone(),
        }
    }

    fn render_key_expiry_controls(tmpl: &mut TemplateBuffer<'_>, expiry_radio_name: &str) {
        tmpl << html!(
            div(class = "form-section") {
                input(type = "checkbox");
                label: "Unlisted";
                input(type = "text", placeholder = "key");
                button(type = "button", onclick = "generateKey(event)"): "Generate";
            }
            div(class = "form-section") {
                input(type = "checkbox");
                label: "Expires";
                div(class = "expiry-presets") {
                    div {
                        label {
                            input(type = "radio", name = expiry_radio_name, value = "1h");
                            : " 1h";
                        }
                        label {
                            input(type = "radio", name = expiry_radio_name, value = "4h");
                            : " 4h";
                        }
                        label {
                            input(type = "radio", name = expiry_radio_name, value = "1d");
                            : " 1d";
                        }
                        label {
                            input(type = "radio", name = expiry_radio_name, value = "1w");
                            : " 1w";
                        }
                        label {
                            input(type = "radio", name = expiry_radio_name, value = "1mo");
                            : " 1mo";
                        }
                        label {
                            input(type = "radio", name = expiry_radio_name, value = "1y");
                            : " 1y";
                        }
                    }
                    div {
                        label {
                            input(type = "radio", name = expiry_radio_name, value = "custom");
                            : " Custom: ";
                            input(type = "datetime-local", class = "expiry-custom");
                        }
                    }
                }
            }
        );
    }

    fn render_item(&self, tmpl: &mut TemplateBuffer<'_>, info: &AdminObjectInfo) {
        let item = &info.item;
        let objects_url = ItemUrl {
            base_url: self.objects_base_url,
            directory_path: "",
            item_name: item.name.as_ref(),
            unlisted_key: info.unlisted_key.as_deref(),
        };
        let download_url = ItemUrl {
            base_url: self.download_base_url,
            directory_path: "",
            item_name: item.name.as_ref(),
            unlisted_key: info.unlisted_key.as_deref(),
        };
        let objects_url_str = format!("{}", objects_url);
        let key_str = info.unlisted_key.as_deref().unwrap_or("");
        let expires = info.expires.map(|e| e.to_rfc3339()).unwrap_or_default();
        let thumbnail_class = if item.is_thumbnailable {
            "main-link"
        } else {
            "main-link no-thumbnail"
        };
        tmpl << html!(
            li(class = format_args!("{} admin-object", item.item_type)) {
                a(
                    class = thumbnail_class,
                    href = format_args!("{}", download_url)
                ) {
                    @ if item.is_thumbnailable {
                        : ThumbnailImg(objects_url);
                    }
                    span(class = "underlined") {
                        : item.name.as_ref();
                        @ if item.item_type == ItemType::Directory {
                            : "/";
                        }
                    }
                    @ if info.unlisted_key.is_some() {
                        img(src = self.asset_url("hidden.svg"), class = "unlisted", alt = "unlisted", title = "unlisted");
                    }
                }
                div(class = "details-outer") {
                    div(class = "details-inner") {
                        @ match &info.ownership {
                            ObjectOwnership::Owned => { span(class = "ownership"): "Owned"; }
                            ObjectOwnership::Linked(path) => {
                                span(class = "ownership") {
                                    : "Linked: ";
                                    : Ellipsis::new(path.as_str(), 30, 25);
                                }
                            }
                        }
                        @ if item.item_type != ItemType::Directory {
                            span(class = "size"): format_size(item.file_size, BINARY);
                        }
                        @ if let Some(modified) = item.modified {
                            : FormatedIsoTimestamp(modified.with_timezone(self.display_timezone));
                        }
                        @ if let Some(expires) = info.expires {
                            span(class = "expiry") {
                                : "Expires: ";
                                : FormatedIsoTimestamp(expires.with_timezone(self.display_timezone));
                            }
                        }
                    }
                    div(class = "details-inner") {
                        button(
                            type = "button",
                            onclick = "copyItemUrl(this)"
                        ): "Copy URL";
                        button(
                            type = "button",
                            onclick = "openEdit(this)",
                            data_id = item.name.as_ref(),
                            data_objects_url = objects_url_str.as_str(),
                            data_key = key_str,
                            data_expires = expires.as_str()
                        ): "Edit";
                    }
                }
            }
        );
    }

    fn asset_url(&self, file_name: &'a str) -> AssetUrl<'a> {
        AssetUrl {
            base_url: self.objects_base_url,
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

                form(id = "add-form", autocomplete = "off") {
                    h2: "Add object";

                    div {
                        input(type = "radio", name = "mode", id="mode-upload", value = "upload", checked);
                        label(for = "mode-upload"): "Upload";
                        input(type = "file", name = "file");
                    }

                    div(class = "path-picker") {
                        input(type = "radio", name = "mode", id="mode-link", value = "link");
                        label(for = "mode-link"): "Link";
                        input(type = "text", name = "path", placeholder = "Path to file", autocomplete = "off");
                    }

                    div(class = "form-section") {
                        input(type = "checkbox", name = "override-id");
                        label(for = "object-id"): "ID";
                        input(type = "text", name = "object-id", id="object-id", placeholder = "Override object ID");
                    }

                    |tmpl| AdminListing::render_key_expiry_controls(tmpl, "add-expiry");

                    button(type = "submit"): "Add";
                    span(class = "result");
                }

                details(id = "cache-stats") {
                    summary: "Thumbnail cache stats";
                    table;
                }

            }

            section(id = "edit-overlay", onclick = "editBackgroundClick(event)") {
                form(autocomplete = "off") {
                    h2(id = "edit-title");
                    |tmpl| AdminListing::render_key_expiry_controls(tmpl, "edit-expiry");
                    div {
                        button(type = "button", onclick = "editSave()"): "Save";
                        button(type = "button", onclick = "editDelete()"): "Delete";
                        button(type = "button", onclick = "closeEdit()"): "Cancel";
                    }
                    span(id = "edit-result");
                }
            }

        );
    }
}
