use chrono::{DateTime, Datelike, TimeZone, Timelike};
use horrorshow::{RenderOnce, TemplateBuffer, html};
use std::fmt::{Display, Formatter};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, PercentEncode, utf8_percent_encode};

const PERCENT_ENCODING_CHARSET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'_')
    .remove(b'-')
    .remove(b'.');

pub fn url_encode(s: &str) -> PercentEncode<'_> {
    utf8_percent_encode(s, PERCENT_ENCODING_CHARSET)
}

/// URL for a download-side item: `{base_url}[/{dir}]/{name}[?key={key}]`
#[derive(Clone)]
pub struct ItemUrl<'a> {
    pub base_url: &'a str,
    pub directory_path: &'a str,
    pub item_name: &'a str,
    pub unlisted_key: Option<&'a str>,
}

impl Display for ItemUrl<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.base_url)?;
        if !self.directory_path.is_empty() {
            write!(f, "/{}", url_encode(self.directory_path))?;
        }
        if !self.item_name.is_empty() {
            write!(f, "/{}", url_encode(self.item_name))?;
        }
        if let Some(unlisted_key) = self.unlisted_key {
            write!(f, "?key={}", unlisted_key)?;
        }
        Ok(())
    }
}

impl RenderOnce for ItemUrl<'_> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        tmpl << format_args!("{}", self);
    }
}

impl ItemUrl<'_> {
    pub fn next_qs_separator(&self) -> char {
        if self.unlisted_key.is_some() {
            '&'
        } else {
            '?'
        }
    }
}

/// Renders a thumbnail `<img>` with srcset at 64/128/256px.
/// The `ItemUrl` is used as the item's access URL; `?key=` is included if the item is unlisted.
pub struct ThumbnailImg<'a>(pub ItemUrl<'a>);

impl RenderOnce for ThumbnailImg<'_> {
    fn render_once(self, tmpl: &mut TemplateBuffer<'_>) {
        let base = format!("{}", self.0);
        let sep = self.0.next_qs_separator();
        let u64 = format!("{}{}mode=thumbnail&size=64", base, sep);
        let u128 = format!("{}{}mode=thumbnail&size=128", base, sep);
        let u256 = format!("{}{}mode=thumbnail&size=256", base, sep);
        tmpl << html!(
            img(
                src = u64.as_str(),
                srcset = format_args!("{} 64w, {} 128w, {} 256w", u64, u128, u256),
                sizes = "4em",
                loading = "lazy",
                alt = ""
            );
        );
    }
}

pub struct FormatedIsoTimestamp<Tz: TimeZone>(pub DateTime<Tz>);

impl<Tz> RenderOnce for FormatedIsoTimestamp<Tz>
where
    Tz: TimeZone,
{
    fn render_once(self, tmpl: &mut horrorshow::prelude::TemplateBuffer<'_>) {
        let ts = self.0.fixed_offset();

        let y = ts.year();
        let m = ts.month();
        let d = ts.day();
        let h = ts.hour();
        let minute = ts.minute();
        let s = ts.second();
        let offset = ts.offset();

        tmpl << html!(
            time(
                datetime = format_args!("{y}-{m:02}-{d:02}T{h:02}:{minute:02}:{s:02}{offset}")
            ) {
                : format_args!("{y}-{m:02}-{d:02}");
                span(class = "separator"): "T";
                : format_args!("{h:02}:{minute:02}:{s:02}");

            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::assert;
    use chrono::TimeZone;
    use horrorshow::Template;

    #[test]
    fn url_encode() {
        // slash, dot, underscore, hyphen preserved; space and & encoded; alphanumeric untouched
        assert!(
            super::url_encode("hello world/a._-b&c=1").to_string() == "hello%20world/a._-b%26c%3D1"
        );
    }

    #[test]
    fn formatted_iso_timestamp_correct_date() {
        // March 15 at UTC+2 — month (3) and day (15) differ, so a month/day mixup is detectable
        use chrono::FixedOffset;
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let dt = tz.with_ymd_and_hms(2024, 3, 15, 10, 30, 45).unwrap();
        let rendered = FormatedIsoTimestamp(dt).into_string().unwrap();
        assert!(
            rendered.contains(r#"datetime="2024-03-15T10:30:45+02:00""#),
            "rendered: {rendered}"
        );
        assert!(rendered.contains(">2024-03-15<"), "rendered: {rendered}");
        assert!(rendered.contains(">10:30:45<"), "rendered: {rendered}");
    }
}
