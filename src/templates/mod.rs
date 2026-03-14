mod admin_listing;
mod breadcrumbs;
mod dir_listing;
mod icons_css;
mod page;
pub mod util;

use std::fmt::{Display, Formatter};

pub use admin_listing::AdminListing;
pub use dir_listing::DirListing;
use horrorshow::{RenderOnce, TemplateBuffer};
pub use icons_css::icons_css;

#[derive(Clone)]
struct AssetUrl<'a> {
    base_url: &'a str,
    file_name: &'a str,
    cache_hash: &'a str,
}

impl Display for AssetUrl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}?mode=assets&cache_hash={}",
            self.base_url, self.file_name, self.cache_hash
        )
    }
}

impl RenderOnce for AssetUrl<'_> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        tmpl << format_args!("{}", self);
    }
}
