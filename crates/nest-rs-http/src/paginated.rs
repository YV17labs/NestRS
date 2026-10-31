//! [`Paginated`] — a page of a collection, linked to the next one.

use std::fmt::Display;

use crate::link::set_next_link;
use crate::response::{IntoResponse, Response};

/// A page of a collection: `body`, and when another page follows, the `Link`
/// to it (RFC 8288, `rel="next"`), as a keyset list answers.
///
/// The target is `<the path the caller addressed>?first=N&after=CURSOR`, a
/// relative reference, so a global prefix and a version survive and the
/// client-controlled `Host` is never named; the last page carries none.
/// Whoever answers it — a route, a self-mount's entry, a route registered by
/// hand — has the header written, and `#[routes]` documents the `Link` and the
/// body off the return type, an alias included.
///
/// ```
/// # use nest_rs_core::module;
/// # use nest_rs_http::{Json, Paginated, Result, controller, header, routes};
/// # use nest_rs_testing::TestApp;
/// #[controller(path = "/tracks")]
/// struct TracksController;
///
/// #[routes]
/// impl TracksController {
///     #[get("/")]
///     #[public]
///     async fn list(&self) -> Result<Paginated<Json<Vec<u32>>>> {
///         Ok(Paginated::new(Json(vec![1, 2]), 2, Some(2)))
///     }
/// }
/// # #[module(providers = [TracksController])]
/// # struct TracksModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = TestApp::for_module::<TracksModule>().await?;
///
/// let page = app.http().get("/tracks").send().await;
/// page.assert_header(header::LINK, "</tracks?first=2&after=2>; rel=\"next\"");
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
#[must_use = "a `Paginated` answers nothing until a route returns it"]
pub struct Paginated<T> {
    body: T,
    next: Option<NextPage>,
}

impl<T> Paginated<T> {
    /// `body`, a page of `first` rows at most, followed by the page starting
    /// after `after` — `None` on the last page.
    pub fn new<C: Display>(body: T, first: u64, after: Option<C>) -> Self {
        Self {
            body,
            next: after.map(|after| NextPage {
                first,
                after: after.to_string(),
            }),
        }
    }
}

/// The next page a [`Paginated`] answer carries until the tail answering it
/// writes the `Link`, which needs the path the caller addressed.
#[derive(Clone, Debug)]
struct NextPage {
    first: u64,
    after: String,
}

impl<T: IntoResponse> IntoResponse for Paginated<T> {
    fn into_response(self) -> Response {
        let mut response = self.body.into_response();
        if let Some(next) = self.next {
            response.extensions_mut().insert(next);
        }
        response
    }
}

/// Write the `Link` a [`Paginated`] answer carries, under the path
/// `caller_path` reads, read only when one is carried.
pub(crate) fn stamp_next_link<'a>(caller_path: impl FnOnce() -> &'a str, response: &mut Response) {
    if let Some(NextPage { first, after }) = response.extensions_mut().remove::<NextPage>() {
        set_next_link(response, caller_path(), first, &after);
    }
}

#[cfg(test)]
mod tests {
    use http::header::LINK;

    use super::*;

    fn stamped(page: Paginated<&'static str>) -> Option<String> {
        let mut response = page.into_response();
        stamp_next_link(|| "/tracks", &mut response);
        response
            .headers()
            .get(LINK)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }

    #[test]
    fn a_page_another_follows_links_it_on_the_callers_path() {
        assert_eq!(
            stamped(Paginated::new("[]", 20, Some("abc"))).as_deref(),
            Some("</tracks?first=20&after=abc>; rel=\"next\""),
        );
    }

    #[test]
    fn the_last_page_links_nothing() {
        assert_eq!(stamped(Paginated::new("[]", 20, None::<&str>)), None);
    }
}
