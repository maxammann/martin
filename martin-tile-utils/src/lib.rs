// This code was partially adapted from https://github.com/maplibre/mbtileserver-rs
// project originally written by Kaveh Karimi and licensed under MIT/Apache-2.0

use actix_http::ContentEncoding;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
    Json,
    Mvt,
    GzipMvt,
    ZlibMvt,
    Unknown,
}

impl DataFormat {
    #[must_use]
    pub fn detect(data: &[u8]) -> Self {
        match data {
            // Compressed prefixes assume MVT content
            v if &v[0..2] == b"\x1f\x8b" => Self::GzipMvt,
            v if &v[0..2] == b"\x78\x9c" => Self::ZlibMvt,
            v if &v[0..8] == b"\x89\x50\x4E\x47\x0D\x0A\x1A\x0A" => Self::Png,
            v if &v[0..6] == b"\x47\x49\x46\x38\x39\x61" => Self::Gif,
            v if &v[0..3] == b"\xFF\xD8\xFF" => Self::Jpeg,
            v if &v[0..4] == b"RIFF" && &v[8..12] == b"WEBP" => Self::Webp,
            v if &v[0..1] == b"{" => Self::Json,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub fn content_type(&self) -> Option<&str> {
        match *self {
            Self::Png => Some("image/png"),
            Self::Jpeg => Some("image/jpeg"),
            Self::Gif => Some("image/gif"),
            Self::Webp => Some("image/webp"),
            Self::Json => Some("application/json"),
            Self::Mvt | Self::GzipMvt | Self::ZlibMvt => Some("application/x-protobuf"),
            Self::Unknown => None,
        }
    }

    #[must_use]
    pub fn content_encoding(&self) -> Option<ContentEncoding> {
        match *self {
            Self::GzipMvt => Some(ContentEncoding::Gzip),
            Self::ZlibMvt => Some(ContentEncoding::Deflate),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::read;

    use super::*;

    #[test]
    fn test_data_format_png() {
        assert_eq!(
            DataFormat::detect(&read("./data/world.png").unwrap()),
            DataFormat::Png
        );
    }

    #[test]
    fn test_data_format_jpg() {
        assert_eq!(
            DataFormat::detect(&read("./data/world.jpg").unwrap()),
            DataFormat::Jpeg
        );
    }

    #[test]
    fn test_data_format_webp() {
        assert_eq!(
            DataFormat::detect(&read("./data/dc.webp").unwrap()),
            DataFormat::Webp
        );
    }
}
