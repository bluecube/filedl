mod dir_listing;
mod page;
pub mod util;

use std::fmt::{Display, Formatter};

pub use dir_listing::DirListing;
use horrorshow::{RenderOnce, TemplateBuffer};

#[derive(Clone)]
struct AssetUrl<'a> {
    download_base_url: &'a str,
    file_name: &'a str,
    cache_hash: &'a str,
}

impl<'a> Display for AssetUrl<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}?mode=internal&cache_hash={}",
            self.download_base_url, self.file_name, self.cache_hash
        )
    }
}

impl<'a> RenderOnce for AssetUrl<'a> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        tmpl << format_args!("{}", self);
    }
}
