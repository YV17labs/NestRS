//! A multipart part, read as it arrives.
//!
//! poem's `Field::bytes()` is the buffered form: the whole part lands in memory
//! before the handler sees any of it, which is fine for a form value and wrong
//! for a file. [`PartExt::into_byte_stream`] is the other half — the same part
//! as a byte stream, so an upload can be piped straight into an object store
//! and never exist whole anywhere.
//!
//! It changes nothing about the ceiling: the part is read through the request
//! body, which the transport edge already caps at
//! [`HttpConfig.max_body_bytes`](crate::HttpConfig). Streaming bounds *memory*,
//! not the request.

use std::io::Result as IoResult;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use poem::web::Field;
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;

/// One multipart part's bytes, as they arrive. Yielded chunks are whatever the
/// transport read — never a whole part.
pub struct PartStream(ReaderStream<Pin<Box<dyn AsyncRead + Send>>>);

impl Stream for PartStream {
    type Item = IoResult<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().0).poll_next(cx)
    }
}

/// Read a multipart part without buffering it.
pub trait PartExt {
    /// The part's bytes as they arrive.
    ///
    /// ```rust,ignore
    /// # use nest_rs::http::PartExt;
    /// let key = format!("{}-{}", Uuid::now_v7(), part.file_name().unwrap_or("upload"));
    /// storage.put_stream(&key, "audio/mpeg", part.into_byte_stream()).await?;
    /// ```
    fn into_byte_stream(self) -> PartStream;
}

impl PartExt for Field {
    fn into_byte_stream(self) -> PartStream {
        PartStream(ReaderStream::new(Box::pin(self.into_async_read())))
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use poem::test::TestClient;
    use poem::web::Multipart;
    use poem::{Result, handler};

    use super::*;

    /// Reads the `file` part as a stream and reports what it saw, so the test
    /// can assert both the bytes and that they arrived in pieces.
    #[handler]
    async fn upload(mut form: Multipart) -> Result<String> {
        let mut chunks = 0usize;
        let mut body = Vec::new();
        while let Some(field) = form.next_field().await? {
            if field.name() != Some("file") {
                continue;
            }
            let mut stream = field.into_byte_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(poem::error::InternalServerError)?;
                chunks += 1;
                body.extend_from_slice(&chunk);
            }
        }
        Ok(format!("{chunks}:{}", body.len()))
    }

    #[tokio::test]
    async fn a_part_streams_its_bytes_without_being_buffered_whole() {
        // Big enough that poem hands it over in several reads — the property
        // that makes this worth having.
        let payload = vec![b'a'; 512 * 1024];
        let form = poem::test::TestForm::new().bytes("file", payload.clone());
        let resp = TestClient::new(upload)
            .post("/")
            .multipart(form)
            .send()
            .await;
        resp.assert_status_is_ok();
        let seen = resp.0.into_body().into_string().await.expect("body");
        let (chunks, bytes) = seen.split_once(':').expect("chunks:bytes");
        assert_eq!(
            bytes.parse::<usize>().expect("byte count"),
            payload.len(),
            "every byte arrives",
        );
        assert!(
            chunks.parse::<usize>().expect("chunk count") > 1,
            "and they arrive in pieces, not one buffer: {seen}",
        );
    }

    #[tokio::test]
    async fn an_empty_part_yields_no_bytes_rather_than_failing() {
        let form = poem::test::TestForm::new().bytes("file", Vec::new());
        let resp = TestClient::new(upload)
            .post("/")
            .multipart(form)
            .send()
            .await;
        resp.assert_status_is_ok();
        let seen = resp.0.into_body().into_string().await.expect("body");
        assert!(seen.ends_with(":0"), "no bytes: {seen}");
    }
}
