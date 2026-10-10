//! Converts between nestrs's HTTP vocabulary and poem's while the transport
//! moves onto the vocabulary family by family. Nothing is lost either way: what
//! one side has no place for rides the other's extensions, privately, and comes
//! back on the return crossing.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use http::StatusCode;
use http_body_util::combinators::BoxBody;
use hyper::upgrade::OnUpgrade;
use poem::web::{LocalAddr, RemoteAddr};

use crate::body::Body;
use crate::endpoint::Endpoint;
use crate::error::{BoxError, HttpError, Result};
use crate::problem::render_error;
use crate::request::{Request, RequestFacts, Routed};
use crate::response::{IntoResponse, Response};

/// What poem keeps private about a request it built — its peer, its upgrade,
/// its route's parameters — emptied of the head nestrs carries, so the way back
/// restores it.
#[derive(Clone)]
struct PoemState(Arc<Mutex<Option<poem::RequestParts>>>);

impl PoemState {
    fn new(parts: poem::RequestParts) -> Self {
        Self(Arc::new(Mutex::new(Some(parts))))
    }

    fn take(self) -> Option<poem::RequestParts> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// hyper's upgrade, cloned before poem moves the original into its private
/// state: either side may take the socket, both await one receiver.
#[derive(Clone)]
struct CarriedUpgrade(OnUpgrade);

/// The route nestrs's router matched, which a poem request has no place for.
#[derive(Clone)]
struct CarriedRoute(Routed);

/// A poem request as nestrs's.
pub fn request_from_poem(request: poem::Request) -> Request {
    let peer = request.remote_addr().as_socket_addr().copied();
    let local = request.local_addr().as_socket_addr().copied();
    let scheme = request.scheme().clone();
    let original_uri = request.original_uri().clone();
    let (mut parts, body) = request.into_parts();
    let (mut head, ()) = http::Request::new(()).into_parts();
    head.method = std::mem::take(&mut parts.method);
    head.uri = std::mem::take(&mut parts.uri);
    head.version = parts.version;
    head.headers = std::mem::take(&mut parts.headers);
    head.extensions = std::mem::take(&mut parts.extensions);
    let route = head
        .extensions
        .remove::<CarriedRoute>()
        .map(|CarriedRoute(route)| route);
    if let Some(CarriedUpgrade(upgrade)) = head.extensions.remove::<CarriedUpgrade>() {
        head.extensions.insert(upgrade);
    }
    head.extensions.insert(PoemState::new(parts));
    let facts = RequestFacts {
        peer,
        local,
        scheme,
        original_uri,
        route,
    };
    Request::from_raw(head, body_from_poem(body), facts)
}

/// A poem request as nestrs's, matched by poem's router at `template` (in
/// nestrs's grammar), each of `names` read off poem's parameters.
pub fn request_from_poem_at(
    request: poem::Request,
    template: &Arc<str>,
    names: &[Arc<str>],
) -> Request {
    let params = names
        .iter()
        .filter_map(|name| {
            let value = request.raw_path_param(name)?;
            Some((Arc::clone(name), value.to_owned()))
        })
        .collect();
    let mut request = request_from_poem(request);
    request.set_route(Arc::clone(template), params);
    request
}

/// A nestrs request as poem's.
pub fn request_to_poem(request: Request) -> poem::Request {
    let (mut head, body, facts) = request.into_raw();
    let mut parts = match head
        .extensions
        .remove::<PoemState>()
        .and_then(PoemState::take)
    {
        Some(mut parts) => {
            parts.method = head.method;
            parts.uri = head.uri;
            parts.version = head.version;
            parts.headers = head.headers;
            parts.extensions = head.extensions;
            parts
        }
        None => {
            let upgrade = head.extensions.get::<OnUpgrade>().cloned();
            // poem reads the URI as sent off the head, then takes the current one.
            let current = std::mem::replace(&mut head.uri, facts.original_uri);
            let mut parts = poem::RequestParts::from((
                head,
                LocalAddr(addr(facts.local)),
                RemoteAddr(addr(facts.peer)),
                facts.scheme,
            ));
            parts.uri = current;
            if let Some(upgrade) = upgrade {
                parts.extensions.insert(CarriedUpgrade(upgrade));
            }
            parts
        }
    };
    if let Some(route) = facts.route {
        parts.extensions.insert(CarriedRoute(route));
    }
    poem::Request::from_parts(parts, body_to_poem(body))
}

/// A poem response as nestrs's.
pub fn response_from_poem(response: poem::Response) -> Response {
    let (head, body) = http::Response::<BoxBody<Bytes, io::Error>>::from(response).into_parts();
    Response::from_parts(head, Body::from_boxed(body))
}

/// A nestrs response as poem's.
pub fn response_to_poem(response: Response) -> poem::Response {
    let (head, body) = response.into_parts();
    poem::Response::from_parts(
        poem::ResponseParts {
            status: head.status,
            version: head.version,
            headers: head.headers,
            extensions: head.extensions,
        },
        body_to_poem(body),
    )
}

/// A poem error as nestrs's: an [`HttpError`] that crossed into poem comes
/// back whole, with what poem's side added to it; any other answers as poem
/// renders it, its `set_data` values kept inside it — they reach the response,
/// never [`HttpError::extensions`].
pub fn error_from_poem(error: poem::Error) -> HttpError {
    let status = error.status();
    let Some(carried) = error.downcast_ref::<Carried>() else {
        return HttpError::wrapping(Box::new(error), status, render_poem);
    };
    let taken = carried.take();
    // poem hands its error's extensions over only on the response it renders;
    // the emptied carrier renders an empty one.
    let added = error.into_response().into_parts().0.extensions;
    let mut error = taken.unwrap_or_else(|| HttpError::from_status(status));
    error.extensions_mut().extend(added);
    error
}

/// A nestrs error as poem's: a poem error that crossed into nestrs goes back
/// whole when nestrs added nothing to it; any other is carried, rendered by
/// nestrs when poem answers it. poem's `downcast_ref` and `is` see only the
/// carrier, and its `source()` ends there: a poem layer matching errors by
/// type misses a carried one.
pub fn error_to_poem(error: HttpError) -> poem::Error {
    match error.into_wrapped::<poem::Error>() {
        Ok(error) => error,
        Err(error) => poem::Error::from(Carried {
            status: error.status(),
            error: Mutex::new(Some(error)),
        }),
    }
}

/// The [`HttpError`] a poem error carries, taken out to be rendered as nestrs
/// renders it: poem's own rendering would replace the response's extensions with
/// its error's.
pub(crate) fn take_carried(error: poem::Error) -> std::result::Result<HttpError, poem::Error> {
    if error.is::<Carried>() {
        Ok(error_from_poem(error))
    } else {
        Err(error)
    }
}

/// A poem body as nestrs's: both wrap one box, which moves.
pub fn body_from_poem(body: poem::Body) -> Body {
    Body::from_boxed(body.into())
}

/// A nestrs body as poem's: both wrap one box, which moves.
pub fn body_to_poem(body: Body) -> poem::Body {
    poem::Body::from(body.into_boxed())
}

/// A poem endpoint answering nestrs's requests.
pub fn from_poem<E: poem::Endpoint + 'static>(endpoint: E) -> impl Endpoint {
    FromPoem(endpoint)
}

/// A nestrs endpoint answering poem's requests.
pub fn to_poem<E: Endpoint>(endpoint: E) -> impl poem::Endpoint<Output = poem::Response> {
    ToPoem(endpoint)
}

struct FromPoem<E>(E);

impl<E: poem::Endpoint + 'static> Endpoint for FromPoem<E> {
    async fn call(&self, req: Request) -> Result<Response> {
        self.0
            .call(request_to_poem(req))
            .await
            .map(|output| response_from_poem(poem::IntoResponse::into_response(output)))
            .map_err(error_from_poem)
    }
}

struct ToPoem<E>(E);

impl<E: Endpoint> poem::Endpoint for ToPoem<E> {
    type Output = poem::Response;

    async fn call(&self, req: poem::Request) -> poem::Result<poem::Response> {
        self.0
            .call(request_from_poem(req))
            .await
            .map(response_to_poem)
            .map_err(error_to_poem)
    }
}

/// An [`HttpError`] inside a poem error: taken out when it crosses back, or
/// rendered when poem answers it.
struct Carried {
    status: StatusCode,
    error: Mutex<Option<HttpError>>,
}

impl Carried {
    fn take(&self) -> Option<HttpError> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl fmt::Debug for Carried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Carried")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// The whole chain, said once: the cause cannot be lent out of the lock as a
/// `source`, so a log line on poem's side reads it here.
impl fmt::Display for Carried {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.error.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(error) => f.write_str(&nest_rs_core::error_message(error)),
            None => fmt::Display::fmt(&self.status, f),
        }
    }
}

impl std::error::Error for Carried {}

impl poem::error::ResponseError for Carried {
    fn status(&self) -> StatusCode {
        self.status
    }

    fn as_response(&self) -> poem::Response {
        match self.take() {
            Some(error) => response_to_poem(error.into_response()),
            None => poem::Response::builder().status(self.status).finish(),
        }
    }
}

/// A poem error answers as poem renders it, each decode failure in its chain
/// said without its value.
fn render_poem(error: BoxError, status: StatusCode) -> Response {
    match error.downcast::<poem::Error>() {
        Ok(error) => response_from_poem(render_error(*error)),
        // Unreachable: the box was filled with a poem error by `error_from_poem`.
        Err(_) => crate::ProblemDetails::from_status(status).into_response(),
    }
}

fn addr(socket: Option<SocketAddr>) -> poem::Addr {
    socket.map_or_else(poem::Addr::default, poem::Addr::SocketAddr)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hyper::upgrade::OnUpgrade;
    use hyper_util::rt::TokioIo;
    use poem::http::StatusCode;

    use super::*;
    use crate::error::BodyError;
    use crate::testing::{Upgrading, read_exactly};

    #[derive(Debug, Clone, PartialEq)]
    struct Marker(&'static str);

    #[derive(Debug, thiserror::Error, PartialEq)]
    #[error("the kettle is busy")]
    struct Busy;

    /// The request poem's own server builds from hyper's, as today's transport
    /// hands every request to the router.
    fn as_poem_builds_it(request: http::Request<hyper::body::Incoming>) -> poem::Request {
        let peer: SocketAddr = "203.0.113.9:40001".parse().unwrap();
        let local: SocketAddr = "10.0.0.2:443".parse().unwrap();
        poem::Request::from((
            request,
            LocalAddr(poem::Addr::SocketAddr(local)),
            RemoteAddr(poem::Addr::SocketAddr(peer)),
            http::uri::Scheme::HTTPS,
        ))
    }

    #[tokio::test]
    async fn the_bridge_round_trip_is_lossless() {
        let Upgrading { request, mut peer } = Upgrading::open().await;
        let mut sent = as_poem_builds_it(request);
        sent.extensions_mut().insert(Marker("request"));
        *sent.uri_mut() = "/chat/".parse().unwrap();

        let crossed = request_from_poem(sent);
        assert_eq!(crossed.peer_addr(), "203.0.113.9:40001".parse().ok());
        assert_eq!(crossed.local_addr(), "10.0.0.2:443".parse().ok());
        assert_eq!(crossed.scheme(), &http::uri::Scheme::HTTPS);
        assert_eq!(crossed.original_uri(), "/chat");
        assert_eq!(crossed.uri(), "/chat/");
        let back = request_to_poem(crossed);

        assert_eq!(back.extensions().get::<Marker>(), Some(&Marker("request")));
        assert_eq!(
            back.remote_addr().as_socket_addr(),
            "203.0.113.9:40001".parse().ok().as_ref()
        );
        assert_eq!(
            back.local_addr().as_socket_addr(),
            "10.0.0.2:443".parse().ok().as_ref()
        );
        assert_eq!(back.scheme(), &http::uri::Scheme::HTTPS);
        assert_eq!(back.original_uri(), "/chat");
        assert_eq!(back.uri(), "/chat/");
        let upgrade = back.take_upgrade().expect("poem still holds the upgrade");
        peer.switch().await;
        let mut socket = tokio::time::timeout(Duration::from_secs(5), upgrade)
            .await
            .unwrap()
            .unwrap();
        peer.send(b"ping").await;
        assert_eq!(read_exactly(&mut socket, 4).await, b"ping");

        let mut raised = poem::Error::new(Busy, StatusCode::IM_A_TEAPOT);
        raised.set_data(Marker("error"));
        let crossed = error_from_poem(raised);
        assert_eq!(crossed.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(
            crossed.downcast_ref::<Busy>(),
            Some(&Busy),
            "a downcast reaches into poem's error"
        );
        let back = error_to_poem(crossed);
        assert_eq!(back.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(back.data::<Marker>(), Some(&Marker("error")));
        assert_eq!(back.downcast::<Busy>().ok(), Some(Busy));
    }

    #[tokio::test]
    async fn an_http_error_crosses_into_poem_and_comes_back_whole() {
        let mut raised = HttpError::from(BodyError::TooLarge { limit: 8 });
        raised.extensions_mut().insert(Marker("nestrs"));

        let mut crossed = error_to_poem(raised);
        assert_eq!(crossed.status(), StatusCode::PAYLOAD_TOO_LARGE);
        crossed.set_data(7_u32);
        let back = error_from_poem(crossed);

        assert_eq!(back.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(back.extensions().get::<Marker>(), Some(&Marker("nestrs")));
        assert_eq!(
            back.extensions().get::<u32>(),
            Some(&7),
            "what poem's side added is kept"
        );
        assert!(matches!(
            back.downcast::<BodyError>(),
            Ok(BodyError::TooLarge { limit: 8 })
        ));
    }

    #[tokio::test]
    async fn a_poem_error_nestrs_added_to_goes_back_with_both_sides_kept() {
        let crossed_in = || {
            let mut crossed = error_from_poem(poem::Error::new(Busy, StatusCode::IM_A_TEAPOT));
            crossed.extensions_mut().insert(Marker("nestrs"));
            crossed
        };

        let back = error_from_poem(error_to_poem(crossed_in()));
        assert_eq!(back.extensions().get::<Marker>(), Some(&Marker("nestrs")));
        assert_eq!(
            back.downcast_ref::<Busy>(),
            Some(&Busy),
            "the poem cause is still reached"
        );

        let rendered = render_error(error_to_poem(crossed_in()));
        assert_eq!(rendered.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(
            rendered.data::<Marker>(),
            Some(&Marker("nestrs")),
            "what nestrs added reaches the response"
        );
        assert_eq!(
            rendered.into_body().into_string().await.unwrap(),
            "the kettle is busy",
            "the poem cause answers as poem renders it"
        );
    }

    #[tokio::test]
    async fn an_http_error_poem_renders_answers_nestrs_problem_document() {
        let mut raised = HttpError::from_status(StatusCode::UNAUTHORIZED);
        raised.extensions_mut().insert(Marker("challenge"));

        let rendered = render_error(error_to_poem(raised));

        assert_eq!(rendered.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(rendered.content_type(), Some("application/problem+json"));
        assert_eq!(rendered.header("www-authenticate"), Some("Bearer"));
        assert_eq!(rendered.data::<Marker>(), Some(&Marker("challenge")));

        let by_poem =
            error_to_poem(HttpError::from_status(StatusCode::UNAUTHORIZED)).into_response();
        assert_eq!(by_poem.content_type(), Some("application/problem+json"));
        assert_eq!(by_poem.header("www-authenticate"), Some("Bearer"));
    }

    #[tokio::test]
    async fn an_upgrade_request_crossed_into_poem_upgrades_from_either_side() {
        // Awaited on poem's side, as a poem-typed gateway takes it.
        let Upgrading { request, mut peer } = Upgrading::open().await;
        let crossed = request_to_poem(Request::from(request));
        let upgrade = crossed.take_upgrade().expect("poem holds the upgrade");
        peer.switch().await;
        let mut socket = tokio::time::timeout(Duration::from_secs(5), upgrade)
            .await
            .unwrap()
            .unwrap();
        peer.send(b"poem").await;
        assert_eq!(read_exactly(&mut socket, 4).await, b"poem");

        // Awaited on nestrs's side after the request came back from poem.
        let Upgrading { request, mut peer } = Upgrading::open().await;
        let crossed = request_to_poem(Request::from(request));
        assert!(crossed.take_upgrade().is_ok(), "poem holds the upgrade");
        let back = request_from_poem(request_to_poem(request_from_poem(crossed)));
        let upgrade = back
            .extensions()
            .get::<OnUpgrade>()
            .cloned()
            .expect("nestrs holds the upgrade");
        peer.switch().await;
        let mut socket = TokioIo::new(
            tokio::time::timeout(Duration::from_secs(5), upgrade)
                .await
                .unwrap()
                .unwrap(),
        );
        peer.send(b"nest").await;
        assert_eq!(read_exactly(&mut socket, 4).await, b"nest");
    }

    #[tokio::test]
    async fn a_poem_leaf_behind_a_crossing_reads_its_route_parameters() {
        let leaf = Arc::new(poem::endpoint::make_sync(|req: poem::Request| {
            req.raw_path_param("room").unwrap_or("none").to_owned()
        }));
        let crossing = poem::endpoint::make(move |req: poem::Request| {
            let leaf = Arc::clone(&leaf);
            async move {
                let crossed = request_from_poem(req);
                poem::Endpoint::call(&*leaf, request_to_poem(crossed)).await
            }
        });
        let app = poem::Route::new().at("/rooms/:room", crossing);

        let answer = poem::test::TestClient::new(app)
            .get("/rooms/lobby")
            .send()
            .await;

        answer.assert_status_is_ok();
        answer.assert_text("lobby").await;
    }

    #[tokio::test]
    async fn a_route_matched_by_poem_is_read_by_name() {
        let template: Arc<str> = "/rooms/{room}".into();
        let names: Arc<[Arc<str>]> = Arc::from([Arc::<str>::from("room")]);
        let leaf = poem::endpoint::make(move |req: poem::Request| {
            let (template, names) = (template.clone(), names.clone());
            async move {
                let crossed = request_from_poem_at(req, &template, &names);
                format!(
                    "{} {}",
                    crossed.route().unwrap_or("none"),
                    crossed.path_param("room").unwrap_or("none")
                )
            }
        });
        let app = poem::Route::new().at("/rooms/:room", leaf);

        let answer = poem::test::TestClient::new(app)
            .get("/rooms/lobby")
            .send()
            .await;

        answer.assert_text("/rooms/{room} lobby").await;
    }

    #[tokio::test]
    async fn the_route_crosses_into_poem_and_back() {
        let mut sent = Request::builder().finish();
        sent.set_route(
            "/rooms/{room}".into(),
            vec![("room".into(), "lobby".into())],
        );

        let back = request_from_poem(request_to_poem(sent));

        assert_eq!(back.route(), Some("/rooms/{room}"));
        assert_eq!(back.path_param("room"), Some("lobby"));
    }

    #[tokio::test]
    async fn a_response_crosses_both_ways_with_its_head_and_body() {
        let sent = Response::builder()
            .status(http::StatusCode::CREATED)
            .header(
                http::header::LOCATION,
                http::HeaderValue::from_static("/orders/7"),
            )
            .extension(Marker("response"))
            .body("made");

        let back = response_from_poem(response_to_poem(sent));

        assert_eq!(back.status(), http::StatusCode::CREATED);
        assert_eq!(back.headers()[http::header::LOCATION], "/orders/7");
        assert_eq!(back.extensions().get::<Marker>(), Some(&Marker("response")));
        assert_eq!(back.into_body().into_string().await.unwrap(), "made");
    }

    #[tokio::test]
    async fn endpoints_answer_across_the_bridge() {
        let nestrs = crate::endpoint_fn(|req: Request| async move {
            Ok(Response::builder().body(format!("nestrs saw {}", req.uri().path())))
        });
        let answer = poem::test::TestClient::new(to_poem(nestrs))
            .get("/here")
            .send()
            .await;
        answer.assert_text("nestrs saw /here").await;

        let failing = poem::endpoint::make(|_req: poem::Request| async {
            Err::<String, _>(poem::Error::new(Busy, StatusCode::SERVICE_UNAVAILABLE))
        });
        let refused = from_poem(failing)
            .call(Request::builder().finish())
            .await
            .unwrap_err();
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(refused.is::<Busy>());
    }
}
