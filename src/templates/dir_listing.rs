use super::{
    AssetUrl,
    breadcrumbs::BreadcrumbsIterator,
    page::Page,
    util::{FormatedIsoTimestamp, ItemUrl, ThumbnailImg, url_encode},
};
use chrono_tz::Tz;
use horrorshow::{RenderOnce, TemplateBuffer, html};
use humansize::{BINARY, format_size};

use crate::app_data::{AppData, DirListingItem, ItemType};

pub struct DirListing<'a> {
    app_name: &'a str,
    download_base_url: &'a str,
    display_timezone: &'a Tz,
    directory_path: &'a str,
    static_content_hash: &'a str,
    unlisted_key: Option<&'a str>,
    items: Vec<DirListingItem>,
}

impl<'a> DirListing<'a> {
    pub fn new_wrapped(
        app: &'a AppData,
        directory_path: &'a str,
        unlisted_key: Option<&'a str>,
        mut items: Vec<DirListingItem>,
    ) -> Page<'a, Title<'a>, DirListing<'a>> {
        let mut collator = feruca::Collator::default();
        items.sort_unstable_by(|a, b| collator.collate(a.name.as_bytes(), b.name.as_bytes()));

        let dir_listing = DirListing {
            app_name: app.get_app_name(),
            download_base_url: app.get_download_base_url(),
            display_timezone: app.get_display_timezone(),
            directory_path,
            static_content_hash: app.get_static_content_hash(),
            unlisted_key,
            items,
        };
        Page {
            download_base_url: app.get_download_base_url(),
            title: Title::new(&dir_listing),
            content: dir_listing,
            static_content_hash: app.get_static_content_hash(),
            display_timezone: app.get_display_timezone(),
        }
    }

    fn render_breadcrumbs(&self, tmpl: &mut TemplateBuffer<'_>) {
        let (key1, key2) = if let Some(key) = self.unlisted_key {
            ("?key=", key)
        } else {
            ("", "")
        };
        tmpl << html!(
            @ for crumb in BreadcrumbsIterator::new(self.directory_path) {
                : "/";
                a(href = format_args!("{}/{}{}{}", self.download_base_url, url_encode(crumb.link_url), key1, key2)): crumb.name;
            }
        );
    }

    fn render_item(&self, tmpl: &mut TemplateBuffer<'_>, item: &DirListingItem) {
        let url = ItemUrl::new(self, item);
        tmpl << html!(
            li(class = format_args!("{}", item.item_type)) {
                a(class = format_args!(
                    "main-link{}",
                    if item.is_thumbnailable {
                        ""
                    } else {
                        " no-thumbnail"
                    }
                    ), href = url.clone()) {
                    @ if item.is_thumbnailable {
                        : ThumbnailImg(url.clone());
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
                        @ if item.item_type != ItemType::Directory {
                            span(class="size") {
                                : format_size(item.file_size, BINARY)
                            }
                        }
                        @ if let Some(modified) = item.modified {
                            : FormatedIsoTimestamp(modified.with_timezone(self.display_timezone))
                        }
                    }
                    a(class = "download", href = format_args!("{}{}mode=download", url, url.next_qs_separator())) {
                        img(src = self.asset_url("download.svg"), alt = "Download", title = "Download");
                    }
                }
            }
        )
    }

    fn asset_url(&self, file_name: &'a str) -> AssetUrl<'a> {
        AssetUrl {
            download_base_url: self.download_base_url,
            file_name,
            cache_hash: self.static_content_hash,
        }
    }
}

impl RenderOnce for DirListing<'_> {
    fn render_once(self, tmpl: &mut horrorshow::prelude::TemplateBuffer<'_>) {
        let self_url = ItemUrl::without_item(&self);
        tmpl << html!(
            nav {
                @ if !self.app_name.is_empty() {
                    div(class = "app-name"): self.app_name;
                }
                h1(class = "breadcrumbs") {
                    a(href = self.download_base_url) {
                        img(src = self.asset_url("home.svg"), alt = "Home", title = "Home");
                    }
                    |tmpl| self.render_breadcrumbs(tmpl);
                    @ if self.unlisted_key.is_some() {
                        img(src = self.asset_url("hidden.svg"), class = "unlisted", alt = "unlisted directory", title = "unlisted directory");
                    }
                }
            }

            section(id = "content") {
                @ if self.items.is_empty() {
                    div(class = "empty-dir-listing"): "No data";
                }

                @ if !self.items.is_empty() {
                    @ if !self.directory_path.is_empty() {
                        div(class = "download-all") {
                            a (
                                href = format_args!(
                                    "{}{}mode=download",
                                    self_url,
                                    self_url.next_qs_separator(),
                                )
                            ) {
                              : "Download all";
                              img(src = self.asset_url("download.svg"), alt = "");
                            }
                        }
                    }
                    ul(class = "dir-listing") {
                        @ for item in self.items.iter() {
                            |tmpl| self.render_item(tmpl, item)
                        }
                    }
                }
            }

            section(id = "gallery") {
                a(href = "#", class = "close") {
                        img(src = self.asset_url("close.svg"), alt = "Close gallery", title = "Close gallery");
                    }
                div(class = "placeholder");
                div(class = "img-wrap") {
                    a(href = "#", class = "prev") {
                        img(src = self.asset_url("arrow_back.svg"), alt = "Previous image", title = "Previous image");
                    }
                    a(href = "#", class = "next") {
                        img(src = self.asset_url("arrow_forward.svg"), alt = "Next image", title = "Next image");
                    }
                    img(src = "data:,", class="main", alt = "Gallery image");
                    progress;
                }
                div(class = "info") {
                    span(class = "description");
                    a(href = "#", class = "download"): "Download";
                }
            }
        );
    }
}

// Local constructors for ItemUrl that take DirListing-specific arguments.
impl<'a> ItemUrl<'a> {
    fn new(dl: &'a DirListing, item: &'a DirListingItem) -> Self {
        ItemUrl {
            base_url: dl.download_base_url,
            directory_path: dl.directory_path,
            item_name: &item.name,
            unlisted_key: dl.unlisted_key,
        }
    }

    fn without_item(dl: &'a DirListing) -> Self {
        ItemUrl {
            base_url: dl.download_base_url,
            directory_path: dl.directory_path,
            item_name: "",
            unlisted_key: dl.unlisted_key,
        }
    }
}

pub struct Title<'a> {
    pub app_name: &'a str,
    pub directory_path: &'a str,
}

impl<'a> Title<'a> {
    fn new(dir_listing: &DirListing<'a>) -> Title<'a> {
        Title {
            app_name: dir_listing.app_name,
            directory_path: dir_listing.directory_path,
        }
    }
}

impl RenderOnce for Title<'_> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        if !self.directory_path.is_empty() {
            tmpl << format_args!("{} - {}", self.directory_path, self.app_name);
        } else {
            tmpl << self.app_name;
        }
    }
}
