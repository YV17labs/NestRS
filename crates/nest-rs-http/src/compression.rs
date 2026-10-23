//! [`Compression`]: the content codings around the edge — a request body its
//! `Content-Encoding` names, decoded before anything reads it; a response
//! compressed with the coding its request's `Accept-Encoding` weighs highest
//! (`accept_encoding.rs`), unless it holds no content to encode. The codecs are
//! async-compression's.
#![expect(
    clippy::disallowed_types,
    reason = "async-compression's codecs are this file's, which wraps them"
)]

use std::pin::Pin;

use async_compression::Level;
use async_compression::tokio::bufread::{
    BrotliDecoder, BrotliEncoder, DeflateDecoder, DeflateEncoder, GzipDecoder, GzipEncoder,
    ZstdDecoder, ZstdEncoder,
};
use http::header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
};
use http::{HeaderValue, StatusCode};
use tokio::io::AsyncRead;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::accept_encoding::{AcceptEncoding, Coding};
use crate::body::Body;
use crate::endpoint::Endpoint;
use crate::error::Result;
use crate::request::Request;
use crate::response::Response;
use crate::vary::vary_on;

/// `inner` behind the content codings.
pub(crate) struct Compression<E>(pub(crate) E);

impl<E: Endpoint> Endpoint for Compression<E> {
    async fn call(&self, mut req: Request) -> Result<Response> {
        if let Some(coding) = req
            .headers()
            .get(CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .and_then(Coding::named)
        {
            let body = req.take_body();
            req.set_body(decoded(body, coding));
        }
        let coding = AcceptEncoding::of(req.headers()).and_then(|offered| offered.negotiate());
        let mut resp = self.0.call(req).await?;
        if !compressible(&resp) {
            return Ok(resp);
        }
        vary_on(resp.headers_mut(), ACCEPT_ENCODING);
        if let Some(coding) = coding {
            let headers = resp.headers_mut();
            headers.append(CONTENT_ENCODING, HeaderValue::from_static(coding.token()));
            headers.remove(CONTENT_LENGTH);
            let body = resp.take_body();
            resp.set_body(encoded(body, coding));
        }
        Ok(resp)
    }
}

/// Whether `resp` may be compressed: it has content to encode, none encoded
/// already, is no part of a representation — `Content-Range` counts unencoded
/// bytes (RFC 9110 §14.4) — and is no event stream, whose events a compressor
/// would hold back until its buffer fills.
fn compressible(resp: &Response) -> bool {
    let status = resp.status();
    let bodiless = status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED;
    let partial =
        status == StatusCode::PARTIAL_CONTENT || resp.headers().contains_key(CONTENT_RANGE);
    let event_stream = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("text/event-stream"));
    !bodiless && !partial && !event_stream && !resp.headers().contains_key(CONTENT_ENCODING)
}

/// `body` compressed with `coding`: brotli at its fastest level, which a
/// response's latency affords, the others at their default.
fn encoded(body: Body, coding: Coding) -> Body {
    let read = StreamReader::new(body.into_stream());
    let encoder: Pin<Box<dyn AsyncRead + Send>> = match coding {
        Coding::Brotli => Box::pin(BrotliEncoder::with_quality(read, Level::Fastest)),
        Coding::Deflate => Box::pin(DeflateEncoder::with_quality(read, Level::Default)),
        Coding::Gzip => Box::pin(GzipEncoder::with_quality(read, Level::Default)),
        Coding::Zstd => Box::pin(ZstdEncoder::with_quality(read, Level::Default)),
    };
    Body::from_stream(ReaderStream::new(encoder))
}

/// `body` decoded from `coding`.
fn decoded(body: Body, coding: Coding) -> Body {
    let read = StreamReader::new(body.into_stream());
    let decoder: Pin<Box<dyn AsyncRead + Send>> = match coding {
        Coding::Brotli => Box::pin(BrotliDecoder::new(read)),
        Coding::Deflate => Box::pin(DeflateDecoder::new(read)),
        Coding::Gzip => Box::pin(GzipDecoder::new(read)),
        Coding::Zstd => Box::pin(ZstdDecoder::new(read)),
    };
    Body::from_stream(ReaderStream::new(decoder))
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt;

    use super::*;

    const TEXT: &str = "nestrs compression payload, nestrs compression payload";

    /// An endpoint answering the body it read, as text.
    fn echo() -> Compression<impl Endpoint> {
        Compression(crate::endpoint_fn(|req: Request| async move {
            let encoding = req.header(CONTENT_ENCODING).unwrap_or("-").to_owned();
            let text = req.into_body().into_string().await?;
            Ok(Response::builder().body(format!("{encoding} {text}")))
        }))
    }

    async fn compressed(coding: Coding) -> Vec<u8> {
        encoded(Body::from(TEXT), coding)
            .into_bytes()
            .await
            .expect("encoded")
            .to_vec()
    }

    /// Each coding the transport answers with is one a request body may
    /// arrive in, and both directions agree.
    #[tokio::test]
    async fn a_body_encoded_in_each_coding_decodes_to_itself() {
        for coding in Coding::OFFERED {
            let body = Body::from(compressed(coding).await);
            assert_eq!(
                decoded(body, coding).into_string().await.expect("decoded"),
                TEXT,
                "{}",
                coding.token()
            );
        }
    }

    /// A request body arriving in a coding is read decoded, its
    /// `Content-Encoding` left as the client sent it; one in a coding the
    /// transport does not speak, or spelt otherwise, is read as sent.
    #[tokio::test]
    async fn a_request_body_in_a_coding_is_decoded_before_anything_reads_it() {
        let gzip = compressed(Coding::Gzip).await;
        for (encoding, sent, read) in [
            ("gzip", gzip.clone(), format!("gzip {TEXT}")),
            (
                "identity",
                TEXT.as_bytes().to_vec(),
                format!("identity {TEXT}"),
            ),
        ] {
            let request = Request::builder()
                .header(CONTENT_ENCODING, HeaderValue::from_static(encoding))
                .body(sent);
            let resp = echo().call(request).await.expect("answered");
            assert_eq!(resp.into_body().into_string().await.unwrap(), read);
        }
        let mut raw = Vec::new();
        GzipDecoder::new(&gzip[..])
            .read_to_end(&mut raw)
            .await
            .expect("the test's own gzip");
        assert_eq!(raw, TEXT.as_bytes());
    }

    /// A compressed answer names its coding and drops the length of the
    /// content it no longer carries.
    #[tokio::test]
    async fn a_compressed_answer_names_its_coding_and_no_length() {
        let request = Request::builder()
            .header(ACCEPT_ENCODING, HeaderValue::from_static("br"))
            .finish();
        let resp = echo().call(request).await.expect("answered");
        assert_eq!(resp.headers()[CONTENT_ENCODING], "br");
        assert!(!resp.headers().contains_key(CONTENT_LENGTH));
        assert_eq!(resp.headers()[http::header::VARY], "accept-encoding");
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let mut text = String::new();
        BrotliDecoder::new(&bytes[..])
            .read_to_string(&mut text)
            .await
            .expect("brotli");
        assert_eq!(text, "- ");
    }
}
