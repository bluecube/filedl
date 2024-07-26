use std::fmt::{Display, Write};

use super::{
    page::Page,
    util::{url_encode, FormatedIsoTimestamp},
};
use chrono_tz::Tz;
use horrorshow::{html, labels_sep_by, RenderOnce, TemplateBuffer};
use humansize::{format_size, BINARY};

use crate::{
    app_data::{AppData, DirListingItem},
    breadcrumbs::BreadcrumbsIterator,
};

pub struct DirListing<'a> {
    // TODO: URL encode directory path and items name on construction
    app_name: &'a str,
    download_base_url: &'a str,
    display_timezone: &'a Tz,
    directory_path: &'a str,
    items: Vec<DirListingItem>,
}

impl<'a> DirListing<'a> {
    pub fn new_wrapped(
        app: &'a AppData,
        directory_path: &'a str,
        mut items: Vec<DirListingItem>,
    ) -> Page<'a, Title<'a>, DirListing<'a>> {
        let mut collator = feruca::Collator::default();
        items.sort_unstable_by(|a, b| collator.collate(a.name.as_bytes(), b.name.as_bytes()));

        let dir_listing = DirListing {
            app_name: app.get_app_name(),
            download_base_url: app.get_download_base_url(),
            display_timezone: app.get_display_timezone(),
            directory_path,
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
        tmpl << html!(
            @ for crumb in BreadcrumbsIterator::new(self.directory_path) {
                : "/";
                a(href = format_args!("{}/{}", url_encode(&self.download_base_url), url_encode(crumb.link_url))): crumb.name;
            }
        );
    }

    fn render_item(&self, tmpl: &mut TemplateBuffer<'_>, item: &DirListingItem) {
        let url = ItemUrl::new(self, item);
        tmpl << html!(
            li(class = format!("{}", item.item_type)) {
                a(class = "main-link", href = url.clone()) {
                    @ if item.item_type.is_thumbnailable() {
                        img(
                            class = "thumbnail",
                            src = url.thumbnail(64, None),
                            srcset = labels_sep_by!(
                                ",";
                                format_args!("{} {}w", url.thumbnail(64, None), 64),
                                format_args!("{} {}w", url.thumbnail(128, None), 128),
                                format_args!("{} {}w", url.thumbnail(256, None), 256)
                            ),
                            sizes = "4em",
                            loading = "lazy"
                        );
                    }
                    span(class = "underlined") {
                        : item.name.as_ref();
                        @ if item.item_type.is_directory() {
                            : "/";
                        }
                    }
                }
                div(class = "details1") {
                    div(class = "details2") {
                        @ if !item.item_type.is_directory() {
                            span(class="size") {
                                : format_size(item.file_size, BINARY)
                            }
                        }
                        @ if let Some(modified) = item.modified {
                            : FormatedIsoTimestamp(modified.with_timezone(self.display_timezone))
                        }
                    }
                    a(class = "download", href = format_args!("{}{}mode=download", url, url.next_qs_separator())): "Download";
                }
            }
        )
    }
}

impl<'a> RenderOnce for DirListing<'a> {
    fn render_once(self, tmpl: &mut horrorshow::prelude::TemplateBuffer<'_>) {
        tmpl << html!(
            nav {
                @ if !self.app_name.is_empty() {
                    div(class = "app-name"): self.app_name;
                }
                h1(class = "breadcrumbs") {
                    a(class = "home", href = format_args!("{}", url_encode(self.download_base_url))): "home";
                    |tmpl| self.render_breadcrumbs(tmpl)
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
                                    "{}/{}?mode=download",
                                    url_encode(self.download_base_url),
                                    url_encode(self.directory_path)
                                )
                            ) : "Download all";
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
                a(href = "#", class = "close"): "Close";
                div(class = "placeholder");
                div(class = "img-wrap") {
                    a(href = "#", class = "prev"): "Previous";
                    a(href = "#", class = "next"): "Next";
                    img(src = "data:,", alt = "Gallery image");
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

#[derive(Clone)]
struct ItemUrl<'a> {
    download_base_url: &'a str,
    directory_path: &'a str,
    item_name: &'a str,
    unlisted_key: &'a str,
}

impl<'a> Display for ItemUrl<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/", url_encode(self.download_base_url))?;
        if !self.directory_path.is_empty() {
            write!(f, "{}/", url_encode(self.directory_path))?;
        }
        write!(f, "{}", url_encode(self.item_name))?;
        if !self.unlisted_key.is_empty() {
            write!(f, "?unlisted_key={}", self.unlisted_key)?;
        }

        Ok(())
    }
}

impl<'a> RenderOnce for ItemUrl<'a> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        tmpl << format_args!("{}", self);
    }
}

impl<'a> ItemUrl<'a> {
    fn new(dl: &'a DirListing, item: &'a DirListingItem) -> Self {
        ItemUrl {
            download_base_url: dl.download_base_url,
            directory_path: dl.directory_path,
            item_name: &item.name,
            unlisted_key: "",
        }
    }

    fn thumbnail(&self, resolution: u32, cache_hash: Option<u64>) -> ThumbnailUrl<'a> {
        ThumbnailUrl {
            item: self.clone(),
            resolution,
            cache_hash,
        }
    }

    fn next_qs_separator(&self) -> char {
        if self.unlisted_key.is_empty() {
            '?'
        } else {
            '&'
        }
    }
}

struct ThumbnailUrl<'a> {
    item: ItemUrl<'a>,
    resolution: u32,
    cache_hash: Option<u64>,
}

impl<'a> Display for ThumbnailUrl<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.item)?;
        f.write_char(self.item.next_qs_separator())?;
        write!(f, "mode=thumb{}", self.resolution)?;
        if let Some(hash) = self.cache_hash {
            write!(f, "&cache_hash={:08x}", hash)?;
        }

        Ok(())
    }
}

impl<'a> RenderOnce for ThumbnailUrl<'a> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        tmpl << format_args!("{}", self);
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

impl<'a> RenderOnce for Title<'a> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        if !self.directory_path.is_empty() {
            tmpl << format_args!("{} - {}", self.directory_path, self.app_name);
        } else {
            tmpl << self.app_name;
        }
    }
}
