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
                datetime = format_args!("{y}-{m:02}-{d:02}T{h:02}:{minute:02}:{s:02}+{offset}")
            ) {
                : format_args!("{y}-{m:02}-{d:02}");
                span(class = "separator"): "T";
                : format_args!("{h:02}:{minute:02}:{s:02}");

            }
        );
    }
}
