//! [`media_type`] — the `Content-Type` a file is served with, read off its
//! extension and never off its bytes: what `X-Content-Type-Options: nosniff`
//! asks a browser to trust.

/// What a file whose extension is not listed is served as: downloaded, never
/// rendered or run.
const UNKNOWN: &str = "application/octet-stream";

/// The media type IANA registers for `name`'s extension, among the files a
/// built front end and its public folder hold; [`UNKNOWN`] for any other.
/// Text is declared UTF-8, and JavaScript is `text/javascript` (RFC 9239).
pub(crate) fn media_type(name: &str) -> &'static str {
    let Some((_, extension)) = name.rsplit_once('.') else {
        return UNKNOWN;
    };
    match extension.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/vnd.microsoft.icon",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => UNKNOWN,
    }
}

/// Whether `media_type` is an HTML document — an entry point no bundler
/// fingerprints, so it is revalidated whatever lifetime the other files get.
pub(crate) fn is_html(media_type: &str) -> bool {
    media_type.starts_with("text/html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_is_read_off_the_extension_whatever_its_case() {
        assert_eq!(media_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(media_type("app.JS"), "text/javascript; charset=utf-8");
        assert_eq!(media_type("app.css"), "text/css; charset=utf-8");
        assert_eq!(media_type("security.txt"), "text/plain; charset=utf-8");
        assert_eq!(media_type("logo.svg"), "image/svg+xml");
        assert_eq!(media_type("font.woff2"), "font/woff2");
    }

    #[test]
    fn an_unknown_or_missing_extension_is_bytes() {
        assert_eq!(media_type("archive.tar.xz"), UNKNOWN);
        assert_eq!(media_type("apple-app-site-association"), UNKNOWN);
        assert_eq!(media_type(""), UNKNOWN);
    }

    #[test]
    fn only_an_html_document_is_html() {
        assert!(is_html(media_type("about.html")));
        assert!(!is_html(media_type("app.js")));
    }
}
