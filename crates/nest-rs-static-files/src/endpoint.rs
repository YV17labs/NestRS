//! [`StaticFilesEndpoint`] — what answers a request the router left to the
//! files: `GET` and `HEAD`, conditional and ranged, and a single-page app's
//! navigation fallback.

use std::io::{self, Seek, SeekFrom};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_http::Opaque;
use poem::error::{MethodNotAllowedError, NotFoundError, ResponseError};
use poem::http::header::{
    ACCEPT, ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
    IF_RANGE, LAST_MODIFIED, RANGE, VARY,
};
use poem::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use poem::{Body, Endpoint, Request, Response};
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::asset::{Asset, AssetBody, Found};
use crate::byte_range::ByteRange;
use crate::disk::CHUNK;
use crate::files::Files;
use crate::request_path::{PathRefusal, RequestPath};
use crate::validators::{Precondition, Validators};

/// Revalidate before every reuse: an HTML document, the navigation fallback,
/// and every file while no lifetime is set.
const NO_CACHE: HeaderValue = HeaderValue::from_static("no-cache");

const ALLOWED: HeaderValue = HeaderValue::from_static("GET, HEAD");

const BYTES: HeaderValue = HeaderValue::from_static("bytes");

/// What decides whether a miss is a page or a `404`, which a cache must key on.
const FALLBACK_VARY: HeaderValue = HeaderValue::from_static("Accept, Sec-Fetch-Mode");

/// The Fetch metadata header a browser sends with a navigation.
const SEC_FETCH_MODE: HeaderName = HeaderName::from_static("sec-fetch-mode");

/// The files under one mount, served.
#[derive(Clone)]
pub(crate) struct StaticFilesEndpoint(Arc<Served>);

struct Served {
    path: String,
    files: Files,
    /// What every file but an HTML document is cached with.
    cache_control: HeaderValue,
    spa_fallback: bool,
}

impl StaticFilesEndpoint {
    pub(crate) fn new(
        path: String,
        files: Files,
        max_age: Option<Duration>,
        spa_fallback: bool,
    ) -> Self {
        let cache_control = max_age
            .and_then(|age| {
                HeaderValue::try_from(format!("public, max-age={}", age.as_secs())).ok()
            })
            .unwrap_or(NO_CACHE);
        Self(Arc::new(Served {
            path,
            files,
            cache_control,
            spa_fallback,
        }))
    }
}

impl Served {
    /// The path below the mount, without the `/` that joins them; an empty
    /// segment stays, and is refused as one.
    fn below_mount<'a>(&self, path: &'a str) -> &'a str {
        let rest = path.strip_prefix(self.path.as_str()).unwrap_or(path);
        match self.path.as_str() {
            "/" => rest,
            _ => rest.strip_prefix('/').unwrap_or(rest),
        }
    }

    fn cache_control(&self, asset: &Asset) -> HeaderValue {
        match asset.html {
            true => NO_CACHE,
            false => self.cache_control.clone(),
        }
    }
}

impl Endpoint for StaticFilesEndpoint {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        let served = &*self.0;
        let read = matches!(*req.method(), Method::GET | Method::HEAD);
        let path = match RequestPath::parse(served.below_mount(req.uri().path())) {
            Ok(path) => path,
            Err(refusal) => return Ok(refused(refusal)),
        };
        // Whether a miss here may be a page — what `Accept` then decides.
        let may_fall_back = read && served.spa_fallback && !path.names_a_file();
        let fallback = may_fall_back && is_navigation(req.headers());
        let found = served.files.find(&path, fallback).await.opaque()?;
        let varies = may_fall_back && matches!(found, Found::Fallback(_) | Found::Missing);
        let mut resp = match found {
            Found::Asset(asset) if read => {
                let cache_control = served.cache_control(&asset);
                answer(asset, &req, cache_control)?
            }
            Found::Asset(_) => method_not_allowed(),
            Found::Fallback(index) => answer(index, &req, NO_CACHE)?,
            Found::Escaped => escaped(),
            Found::Missing => not_found(),
        };
        if varies {
            resp.headers_mut().insert(VARY, FALLBACK_VARY);
        }
        Ok(resp)
    }
}

/// The answer to a `GET` or `HEAD` of `asset`: its preconditions, then its
/// range, then the bytes — none for a `HEAD`.
fn answer(asset: Asset, req: &Request, cache_control: HeaderValue) -> poem::Result<Response> {
    let headers = req.headers();
    if let Some(validators) = &asset.validators {
        match validators.evaluate(headers) {
            Precondition::Proceed => {}
            Precondition::NotModified => return Ok(not_modified(validators, cache_control)),
            Precondition::Failed => return Ok(bare(StatusCode::PRECONDITION_FAILED)),
        }
    }
    let head = req.method() == Method::HEAD;
    // Range is defined for GET alone (RFC 9110 §14.2).
    let range = match headers.get(RANGE).and_then(|range| range.to_str().ok()) {
        Some(range) if !head && range_applies(&asset, headers) => ByteRange::of(range, asset.len),
        _ => ByteRange::Whole,
    };
    let (status, part) = match range {
        ByteRange::Whole => (StatusCode::OK, 0..asset.len),
        ByteRange::Part(part) => (StatusCode::PARTIAL_CONTENT, part),
        ByteRange::Unsatisfiable => {
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(CONTENT_RANGE, format!("bytes */{}", asset.len))
                .finish());
        }
    };
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, asset.content_type)
        .header(CACHE_CONTROL, cache_control)
        .header(ACCEPT_RANGES, BYTES)
        .header(CONTENT_LENGTH, part.end - part.start);
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            CONTENT_RANGE,
            format!("bytes {}-{}/{}", part.start, part.end - 1, asset.len),
        );
    }
    if let Some(validators) = &asset.validators {
        builder = builder.header(ETAG, validators.etag().clone());
        if let Some(last_modified) = validators.last_modified() {
            builder = builder.header(LAST_MODIFIED, last_modified.clone());
        }
    }
    if head {
        return Ok(builder.body(Body::empty()));
    }
    Ok(builder.body(body(asset.body, part).opaque()?))
}

/// Whether the `Range` applies: a file without validators cannot be checked
/// against an `If-Range`, so it is sent whole when one is stated.
fn range_applies(asset: &Asset, headers: &HeaderMap) -> bool {
    match &asset.validators {
        Some(validators) => validators.range_applies(headers),
        None => !headers.contains_key(IF_RANGE),
    }
}

/// `part` of the asset's bytes: a slice of those in memory, or the file
/// streamed from where the part starts.
fn body(source: AssetBody, part: Range<u64>) -> io::Result<Body> {
    match source {
        AssetBody::Bytes(bytes) => {
            let (Ok(start), Ok(end)) = (usize::try_from(part.start), usize::try_from(part.end))
            else {
                return Err(io::Error::from(io::ErrorKind::InvalidInput));
            };
            if start > end || end > bytes.len() {
                return Err(io::Error::from(io::ErrorKind::InvalidInput));
            }
            Ok(Body::from_bytes(bytes.slice(start..end)))
        }
        AssetBody::File(mut file) => {
            if part.start > 0 {
                file.seek(SeekFrom::Start(part.start))?;
            }
            let file = tokio::fs::File::from_std(file).take(part.end - part.start);
            Ok(Body::from_bytes_stream(ReaderStream::with_capacity(
                file, CHUNK,
            )))
        }
    }
}

/// `304`, carrying what a `200` would have said to caches (RFC 9110 §15.4.5).
fn not_modified(validators: &Validators, cache_control: HeaderValue) -> Response {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(ETAG, validators.etag().clone())
        .header(CACHE_CONTROL, cache_control)
        .finish()
}

/// Whether a request is a browser asking for a page: a navigation, or an
/// `Accept` listing `text/html` as acceptable.
fn is_navigation(headers: &HeaderMap) -> bool {
    let navigates = headers
        .get(SEC_FETCH_MODE)
        .is_some_and(|mode| mode.as_bytes().eq_ignore_ascii_case(b"navigate"));
    navigates
        || headers
            .get_all(ACCEPT)
            .iter()
            .filter_map(|accept| accept.to_str().ok())
            .flat_map(|accept| accept.split(','))
            .any(accepts_html)
}

/// Whether one media range is `text/html` with a weight above zero.
fn accepts_html(range: &str) -> bool {
    let mut parts = range.split(';');
    let media = parts.next().unwrap_or_default().trim();
    media.eq_ignore_ascii_case("text/html")
        && !parts.any(|param| {
            param.split_once('=').is_some_and(|(name, weight)| {
                name.trim().eq_ignore_ascii_case("q")
                    && weight.trim().parse::<f32>().is_ok_and(|q| q <= 0.0)
            })
        })
}

/// A path refused before any lookup: `400` when it cannot be read, else the
/// `404` any missing file gets. An attempt to climb out is an operator's
/// concern; a hidden name asked for is the background noise of scanners.
fn refused(refusal: PathRefusal) -> Response {
    if refusal.is_escape_attempt() {
        tracing::warn!(
            target: crate::TARGET,
            reason = refusal.as_str(),
            "refused a path climbing out of the static files' root",
        );
    } else {
        tracing::debug!(
            target: crate::TARGET,
            reason = refusal.as_str(),
            "refused a path naming nothing the static files serve",
        );
    }
    match refusal {
        PathRefusal::Malformed => bare(StatusCode::BAD_REQUEST),
        _ => not_found(),
    }
}

/// A link under the root that leaves what it serves: a deployment defect, so
/// the operator hears of it, and the client gets the `404` of a missing file.
fn escaped() -> Response {
    tracing::warn!(
        target: crate::TARGET,
        reason = "link_outside_root",
        "a link under the static files' root resolves outside what it serves",
    );
    not_found()
}

/// The router's own `404`, word for word, so a miss here says nothing about
/// what is mounted.
fn not_found() -> Response {
    NotFoundError.as_response()
}

fn method_not_allowed() -> Response {
    let mut resp = MethodNotAllowedError.as_response();
    resp.headers_mut().insert(ALLOW, ALLOWED);
    resp
}

fn bare(status: StatusCode) -> Response {
    Response::builder().status(status).finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn a_page_request_is_a_navigation() {
        assert!(is_navigation(&accept(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        )));
        let mut headers = HeaderMap::new();
        headers.insert(SEC_FETCH_MODE, HeaderValue::from_static("navigate"));
        assert!(is_navigation(&headers));
    }

    #[test]
    fn an_api_client_is_not_a_navigation() {
        assert!(!is_navigation(&accept("application/json")));
        assert!(!is_navigation(&accept("*/*")));
        assert!(!is_navigation(&accept("text/html;q=0")));
        assert!(!is_navigation(&HeaderMap::new()));
        let mut headers = HeaderMap::new();
        headers.insert(SEC_FETCH_MODE, HeaderValue::from_static("cors"));
        assert!(!is_navigation(&headers));
    }

    #[test]
    fn a_part_outside_the_bytes_is_an_error_never_a_panic() {
        let bytes = AssetBody::Bytes(bytes::Bytes::from_static(b"abcdef"));
        assert!(body(bytes, 1..3).is_ok());
        let bytes = AssetBody::Bytes(bytes::Bytes::from_static(b"abc"));
        assert!(body(bytes, 2..9).is_err());
    }
}
