//! [`Created`] — a `201` that names what it created.

use std::fmt::Display;

use http::StatusCode;

use crate::location::{caller_path, set_created_location};
use crate::request::Request;
use crate::response::{IntoResponse, Response};

/// A `201 Created` answer: `body`, and the `Location` of what the request
/// created (RFC 9110 §15.3.2).
///
/// The location is `<the path the caller addressed>/<id>`, an absolute-path
/// reference, so a global prefix and a version survive and the
/// client-controlled `Host` is never named. The route answering it writes the
/// header, and `#[routes]` documents the status, the `Location` and the body
/// off the return type.
///
/// ```
/// # use nest_rs_core::module;
/// # use nest_rs_http::{Created, Json, Result, StatusCode, controller, header, routes};
/// # use nest_rs_testing::TestApp;
/// #[controller(path = "/orgs")]
/// struct OrgsController;
///
/// #[routes]
/// impl OrgsController {
///     #[post("/")]
///     #[public]
///     async fn create(&self) -> Result<Created<Json<String>>> {
///         Ok(Created::at(7, Json("acme".to_owned())))
///     }
/// }
/// # #[module(providers = [OrgsController])]
/// # struct OrgsModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = TestApp::for_module::<OrgsModule>().await?;
///
/// let created = app.http().post("/orgs").send().await;
/// created.assert_status(StatusCode::CREATED);
/// created.assert_header(header::LOCATION, "/orgs/7");
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
#[must_use = "a `Created` answers nothing until a route returns it"]
pub struct Created<T> {
    id: String,
    body: T,
}

impl<T> Created<T> {
    /// `body`, created at `id` under the collection the caller addressed.
    pub fn at(id: impl Display, body: T) -> Self {
        Self {
            id: id.to_string(),
            body,
        }
    }
}

/// The id a [`Created`] answer carries until its route writes the
/// `Location`, which needs the request.
#[derive(Clone)]
struct CreatedAt(String);

impl<T: IntoResponse> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        let mut response = self.body.into_response();
        response.set_status(StatusCode::CREATED);
        response.extensions_mut().insert(CreatedAt(self.id));
        response
    }
}

/// Write the `Location` a [`Created`] answer carries, on the path `req`
/// addressed.
pub(crate) fn stamp_location(req: &Request, response: &mut Response) {
    if let Some(CreatedAt(id)) = response.extensions_mut().remove::<CreatedAt>() {
        set_created_location(response, caller_path(req), &id);
    }
}

#[cfg(test)]
mod tests {
    use http::Uri;
    use http::header::LOCATION;

    use super::*;

    #[test]
    fn a_created_answer_is_a_201_whose_location_its_route_writes_on_the_callers_path() {
        let req = Request::builder().uri(Uri::from_static("/orgs")).finish();
        let mut response = Created::at(7, "acme").into_response();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert!(response.headers().get(LOCATION).is_none());

        stamp_location(&req, &mut response);

        assert_eq!(
            response
                .headers()
                .get(LOCATION)
                .map(|value| value.as_bytes()),
            Some(&b"/orgs/7"[..]),
        );
        assert!(response.extensions().get::<CreatedAt>().is_none());
    }

    #[test]
    fn a_response_no_created_answer_built_keeps_its_headers() {
        let req = Request::builder().uri(Uri::from_static("/orgs")).finish();
        let mut response = "plain".into_response();

        stamp_location(&req, &mut response);

        assert!(response.headers().get(LOCATION).is_none());
    }
}
