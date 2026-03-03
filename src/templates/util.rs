use chrono::{DateTime, Datelike, TimeZone, Timelike};
use horrorshow::{RenderOnce, html};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, PercentEncode, utf8_percent_encode};

const PERCENT_ENCODING_CHARSET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'_')
    .remove(b'-')
    .remove(b'.');

pub fn url_encode(s: &str) -> PercentEncode<'_> {
    utf8_percent_encode(s, PERCENT_ENCODING_CHARSET)
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
