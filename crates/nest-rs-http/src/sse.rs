//! Server-Sent Events — the response shape `#[sse]` mounts.
//!
//! A `#[sse]` route returns a stream of [`SseEvent`]: the decorator turns it
//! into the `text/event-stream` response, applies the keep-alive interval, and
//! closes the stream at `<PREFIX>_HTTP__SSE_MAX_CONNECTION_SECS` (4 hours by
//! default, `0` ⇒ unlimited) — a stream authenticates once, so the ceiling
//! bounds stale privileges.
//!
//! The ceiling bounds *emission*; a peer that stops reading is cut by the
//! transport's send deadline, `<PREFIX>_HTTP__SEND_TIMEOUT_SECS`.

use std::fmt;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use bytes::{BufMut, Bytes, BytesMut};
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use http::{HeaderName, HeaderValue, header};
use nest_rs_core::Container;

use crate::config::HttpConfig;
use crate::response::Response;

/// One event on a `text/event-stream` (WHATWG HTML §9.2): a message, and
/// optionally its event type, an id a reconnecting client sends back as
/// `Last-Event-ID`, and how long a client waits before it reconnects.
///
/// Every line break the data holds — `\r\n`, `\n` or `\r`, as the client
/// reads them — opens a `data:` line of its own, so no text can open a field;
/// an id or an event type cannot hold one, so each of their line breaks is
/// dropped.
///
/// ```
/// use std::time::Duration;
///
/// use nest_rs_http::SseEvent;
///
/// let event = SseEvent::message("42 °C")
///     .event_type("reading")
///     .id("7")
///     .retry(Duration::from_secs(3));
/// assert_eq!(
///     event.to_string(),
///     "id: 7\nevent: reading\nretry: 3000\ndata: 42 °C\n\n",
/// );
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    data: String,
    event: String,
    id: String,
    retry: Option<Duration>,
}

impl SseEvent {
    /// A message carrying `data`.
    pub fn message(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            ..Self::default()
        }
    }

    /// The event type a client listens for; `message`, the default, is not
    /// written.
    #[must_use]
    pub fn event_type(mut self, event: impl Into<String>) -> Self {
        self.event = event.into();
        self
    }

    /// The id a reconnecting client sends back as `Last-Event-ID`.
    #[must_use]
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// How long a client waits before it reconnects, in whole milliseconds.
    #[must_use]
    pub fn retry(mut self, after: Duration) -> Self {
        self.retry = Some(after);
        self
    }

    /// The event's frame: its fields, one per line, then a blank line, in one
    /// buffer sized for all of them.
    fn frame(&self) -> Bytes {
        let mut frame = BytesMut::with_capacity(self.frame_len());
        if !self.id.is_empty() {
            field(&mut frame, b"id", &self.id);
        }
        if self.writes_event_type() {
            field(&mut frame, b"event", &self.event);
        }
        if let Some(after) = self.retry {
            frame.put_slice(b"retry: ");
            put_decimal(&mut frame, after.as_millis());
            frame.put_u8(b'\n');
        }
        for line in lines(&self.data) {
            frame.put_slice(b"data: ");
            frame.put_slice(line.as_bytes());
            frame.put_u8(b'\n');
        }
        frame.put_u8(b'\n');
        frame.freeze()
    }

    /// `message`, the type a client assumes, is not written.
    fn writes_event_type(&self) -> bool {
        !self.event.is_empty() && self.event != "message"
    }

    /// The frame's length, counted before it is written: a `\r\n` in the
    /// data counts as two line breaks, and a line break an id or an event type
    /// drops as one written, so a frame never outgrows its buffer.
    fn frame_len(&self) -> usize {
        let line = |name: &str, value_len: usize| name.len() + ": \n".len() + value_len;
        let breaks = self
            .data
            .bytes()
            .filter(|b| matches!(b, b'\r' | b'\n'))
            .count();
        let data_lines = if self.data.is_empty() { 0 } else { breaks + 1 };
        let mut len = data_lines * line("data", 0) + self.data.len() - breaks + "\n".len();
        if !self.id.is_empty() {
            len += line("id", self.id.len());
        }
        if self.writes_event_type() {
            len += line("event", self.event.len());
        }
        if let Some(after) = self.retry {
            len += line("retry", decimal_len(after.as_millis()));
        }
        len
    }
}

/// The frame, as the client receives it.
impl fmt::Display for SseEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.frame()))
    }
}

/// `n` in decimal, its digits written into `frame`, never through a `String`.
fn put_decimal(frame: &mut BytesMut, mut n: u128) {
    let mut digits = [0_u8; DECIMAL_DIGITS_MAX];
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    frame.put_slice(&digits[start..]);
}

/// The number of decimal digits `n` is written with.
fn decimal_len(n: u128) -> usize {
    n.checked_ilog10().map_or(1, |log| log as usize + 1)
}

/// The most decimal digits a `u128` is written with.
const DECIMAL_DIGITS_MAX: usize = u128::MAX.ilog10() as usize + 1;

/// `name: value`, the value's line breaks dropped: a field holds one line.
fn field(frame: &mut BytesMut, name: &[u8], value: &str) {
    frame.put_slice(name);
    frame.put_slice(b": ");
    for part in value.split(['\r', '\n']) {
        frame.put_slice(part.as_bytes());
    }
    frame.put_u8(b'\n');
}

/// `data`'s lines, as a client splits them (WHATWG HTML §9.2.5: `\r\n`, `\n`
/// or `\r`); a line break at the end leaves an empty line after it, so a
/// client reads it back. No data is no line: the client then dispatches
/// nothing.
fn lines(data: &str) -> impl Iterator<Item = &str> {
    let mut rest = (!data.is_empty()).then_some(data);
    std::iter::from_fn(move || {
        let text = rest?;
        match text.find(['\r', '\n']) {
            Some(at) => {
                let skip = if text[at..].starts_with("\r\n") { 2 } else { 1 };
                rest = Some(&text[at + skip..]);
                Some(&text[..at])
            }
            None => {
                rest = None;
                Some(text)
            }
        }
    })
}

/// What a client reads as a comment: a keep-alive, which no listener sees.
const KEEP_ALIVE: &[u8] = b":\n\n";

/// Tells a buffering proxy (nginx) to pass each event on as it is written.
const X_ACCEL_BUFFERING: HeaderName = HeaderName::from_static("x-accel-buffering");

/// What an `#[sse]` handler returns: `-> SseStream`, or
/// `-> Result<SseStream, E>` when opening the stream can fail.
///
/// A named type because an `async fn` on `&self` returning `impl Stream`
/// captures `&self` under the 2024 capture rules, so it is not `'static`.
pub struct SseStream(BoxStream<'static, SseEvent>);

impl SseStream {
    /// Take ownership of any stream of events.
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = SseEvent> + Send + 'static,
    {
        Self(stream.boxed())
    }
}

impl fmt::Debug for SseStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SseStream(..)")
    }
}

/// The two long-lived-connection controls a `#[sse]` route is mounted with,
/// read once from [`HttpConfig`] at mount time.
#[derive(Clone, Copy, Debug)]
pub struct SseSettings {
    keep_alive: Option<Duration>,
    max_connection: Option<Duration>,
}

impl SseSettings {
    /// Resolve from the app's [`HttpConfig`], or its defaults when the
    /// container has none (it then serves no HTTP).
    pub fn resolve(container: &Container) -> Self {
        let config = container
            .get::<HttpConfig>()
            .unwrap_or_else(|| Arc::new(HttpConfig::default()));
        Self {
            keep_alive: config.sse_keep_alive,
            max_connection: config.sse_max_connection,
        }
    }

    /// Wrap a handler's event stream into the response the route serves —
    /// marked [`OpenEndedBody`](crate::OpenEndedBody), so the transport ends it
    /// at the shutdown signal. Emitted by `#[sse]`.
    pub fn respond(&self, stream: SseStream) -> Response {
        let events = stream.0;
        // A ceiling, not an idle timeout: traffic never pushes it out.
        let events = match self.max_connection {
            Some(ttl) => events
                .take_until(async move {
                    tokio::time::sleep(ttl).await;
                    tracing::info!(
                        target: crate::target::HTTP,
                        max_connection_secs = ttl.as_secs(),
                        "closing sse stream: max lifetime reached",
                    );
                })
                .boxed(),
            None => events,
        };
        let mut frames = events.map(|event| event.frame()).boxed();
        if let Some(every) = self.keep_alive {
            let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
            frames = futures_util::stream::poll_fn(move |cx| {
                if let Poll::Ready(frame) = frames.poll_next_unpin(cx) {
                    return Poll::Ready(frame);
                }
                interval
                    .poll_tick(cx)
                    .map(|_| Some(Bytes::from_static(KEEP_ALIVE)))
            })
            .boxed();
        }
        Response::builder()
            .content_type(HeaderValue::from_static("text/event-stream"))
            .header(X_ACCEL_BUFFERING, HeaderValue::from_static("no"))
            .header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
            .extension(crate::OpenEndedBody)
            .body(crate::Body::from_stream(
                frames.map(Ok::<_, std::convert::Infallible>),
            ))
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream;
    use http::StatusCode;

    use super::*;

    async fn served(events: Vec<SseEvent>) -> String {
        let settings = SseSettings {
            keep_alive: None,
            max_connection: None,
        };
        settings
            .respond(SseStream::new(stream::iter(events)))
            .into_body()
            .into_string()
            .await
            .unwrap()
    }

    /// The bytes 6.x's encoder (poem 3.1's `SSE`) wrote for the same events.
    #[tokio::test]
    async fn sse_frames_are_byte_identical_to_6x_for_the_same_events() {
        let body = served(vec![
            SseEvent::message("one"),
            SseEvent::message("two").event_type("tick").id("2"),
            SseEvent::message("first\nsecond"),
            SseEvent::message("crlf\r\nline"),
            SseEvent::message("typed by default").event_type("message"),
            SseEvent::message("").id("9"),
            SseEvent::message("").retry(Duration::from_millis(1500)),
        ])
        .await;
        assert_eq!(
            body,
            "data: one\n\n\
             id: 2\nevent: tick\ndata: two\n\n\
             data: first\ndata: second\n\n\
             data: crlf\ndata: line\n\n\
             data: typed by default\n\n\
             id: 9\n\n\
             retry: 1500\n\n",
        );
    }

    /// A client's reading of a stream (WHATWG HTML §9.2.6): each dispatched
    /// event's type, data and last event id.
    fn dispatched(stream: &str) -> Vec<(String, String, String)> {
        let mut events = Vec::new();
        let (mut kind, mut data, mut last_id) = (String::new(), String::new(), String::new());
        let mut rest = stream;
        while !rest.is_empty() {
            let at = rest.find(['\r', '\n']).unwrap_or(rest.len());
            let line = &rest[..at];
            let skip = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
            rest = rest.get(at + skip..).unwrap_or_default();
            if line.is_empty() {
                if !data.is_empty() {
                    data.pop();
                    let kind = if kind.is_empty() {
                        "message".to_owned()
                    } else {
                        kind.clone()
                    };
                    events.push((kind, std::mem::take(&mut data), last_id.clone()));
                }
                data.clear();
                kind.clear();
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (name, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match name {
                "data" => {
                    data.push_str(value);
                    data.push('\n');
                }
                "event" => kind = value.to_owned(),
                "id" => last_id = value.to_owned(),
                _ => {}
            }
        }
        events
    }

    fn event(kind: &str, data: &str, id: &str) -> (String, String, String) {
        (kind.to_owned(), data.to_owned(), id.to_owned())
    }

    /// WHATWG HTML §9.2.6's examples, and the line breaks a client splits
    /// on, read back by a client as they were sent.
    #[tokio::test]
    async fn a_client_reads_back_the_events_the_whatwg_examples_send() {
        let body = served(vec![
            SseEvent::message("YHOO\n+2\n10"),
            SseEvent::message("first event").id("1"),
            SseEvent::message("second event"),
            SseEvent::message(" third event").event_type("add"),
            SseEvent::message("cr\rlf\ncrlf\r\nend\n"),
        ])
        .await;
        assert_eq!(
            dispatched(&body),
            [
                event("message", "YHOO\n+2\n10", ""),
                event("message", "first event", "1"),
                event("message", "second event", "1"),
                event("add", " third event", "1"),
                event("message", "cr\nlf\ncrlf\nend\n", "1"),
            ],
        );
    }

    #[tokio::test]
    async fn a_carriage_return_in_the_data_cannot_open_a_field_of_its_own() {
        let body = served(vec![SseEvent::message("a\revent: admin")]).await;
        assert_eq!(body, "data: a\ndata: event: admin\n\n");
        assert_eq!(dispatched(&body), [event("message", "a\nevent: admin", "")]);
    }

    #[tokio::test]
    async fn a_line_break_in_an_id_or_an_event_type_cannot_open_a_field() {
        let body = served(vec![
            SseEvent::message("x")
                .id("7\ndata: forged")
                .event_type("tick\r\nretry: 1"),
        ])
        .await;
        assert_eq!(body, "id: 7data: forged\nevent: tickretry: 1\ndata: x\n\n");
    }

    #[test]
    fn a_frame_is_written_into_the_buffer_it_was_sized_for() {
        let exact = [
            SseEvent::message("one"),
            SseEvent::message("two").event_type("transcode").id("01J9"),
            SseEvent::message("first\nsecond\rthird\n"),
            SseEvent::message("typed by default").event_type("message"),
            SseEvent::message("").id("9"),
            SseEvent::message("").retry(Duration::ZERO),
            SseEvent::message("x").retry(Duration::from_millis(1500)),
            SseEvent::message("x").retry(Duration::MAX),
            SseEvent::default(),
        ];
        for event in exact {
            assert_eq!(event.frame().len(), event.frame_len(), "{event:?}");
        }

        let bounded = [
            SseEvent::message("crlf\r\nline\r\n"),
            SseEvent::message("x")
                .id("7\ndata: forged")
                .event_type("tick\r\n"),
        ];
        for event in bounded {
            assert!(event.frame().len() <= event.frame_len(), "{event:?}");
        }
    }

    #[test]
    fn a_retry_is_written_in_whole_milliseconds() {
        for after in [
            Duration::ZERO,
            Duration::from_micros(999),
            Duration::from_millis(7),
            Duration::from_secs(3),
            Duration::MAX,
        ] {
            let frame = SseEvent::message("").retry(after).to_string();
            assert_eq!(frame, format!("retry: {}\n\n", after.as_millis()));
        }
    }

    #[tokio::test]
    async fn a_trailing_line_break_in_the_data_reaches_the_client() {
        let body = served(vec![SseEvent::message("a\n")]).await;
        assert_eq!(body, "data: a\ndata: \n\n");
    }

    #[tokio::test]
    async fn the_response_is_an_open_ended_event_stream_no_proxy_buffers() {
        let settings = SseSettings {
            keep_alive: None,
            max_connection: None,
        };
        let response = settings.respond(SseStream::new(stream::empty()));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(response.headers()["x-accel-buffering"], "no");
        assert!(
            response
                .extensions()
                .get::<crate::OpenEndedBody>()
                .is_some()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_quiet_stream_is_kept_alive_with_a_comment() {
        let settings = SseSettings {
            keep_alive: Some(Duration::from_secs(15)),
            max_connection: Some(Duration::from_secs(40)),
        };
        let body = settings
            .respond(SseStream::new(stream::pending()))
            .into_body()
            .into_string()
            .await
            .unwrap();
        assert_eq!(body, ":\n\n:\n\n");
    }
}
