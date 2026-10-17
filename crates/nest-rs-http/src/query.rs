//! [`Query`]: the request's query string read into a type.

use std::ops::{Deref, DerefMut};

use serde::de::DeserializeOwned;

use crate::error::{QueryError, Result};
use crate::poem_adapter::PoemAdapter;
use crate::request::{FromRequest, Request, RequestBody};

/// The request's query string, decoded as `application/x-www-form-urlencoded`
/// into `T`; an absent query reads as an empty one.
///
/// A query that does not decode into `T` answers `400`, said without the
/// value it held.
///
/// ```
/// use nest_rs_http::{FromRequest, Query, Request, Uri};
///
/// #[derive(serde::Deserialize)]
/// struct Page {
///     after: Option<String>,
///     limit: u32,
/// }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
///
/// let (request, mut body) = Request::builder()
///     .uri(Uri::from_static("/posts?limit=20"))
///     .finish()
///     .split();
/// let Query(page) = Query::<Page>::from_request(&request, &mut body).await?;
/// assert_eq!((page.after, page.limit), (None, 20));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Query<T>(pub T);

impl<T> Query<T> {
    /// The value, out of its wrapper.
    ///
    /// ```
    /// use nest_rs_http::Query;
    ///
    /// assert_eq!(Query(7).into_inner(), 7);
    /// ```
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Query<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<'a, T: DeserializeOwned> FromRequest<'a> for Query<T> {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        PoemAdapter::query(req.uri())
            .map(Query)
            .map_err(|refused| QueryError(refused).into())
    }
}

#[cfg(test)]
mod tests {
    use http::{StatusCode, Uri};
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Page {
        limit: u32,
        after: Option<String>,
    }

    async fn read(uri: &'static str) -> Result<Query<Page>> {
        let (request, mut body) = Request::builder()
            .uri(Uri::from_static(uri))
            .finish()
            .split();
        Query::<Page>::from_request(&request, &mut body).await
    }

    #[tokio::test]
    async fn a_query_decodes_percent_and_plus_into_its_type() {
        let Query(page) = read("/posts?limit=5&after=a%2Fb+c").await.unwrap();
        assert_eq!(
            page,
            Page {
                limit: 5,
                after: Some("a/b c".to_owned()),
            }
        );
    }

    #[tokio::test]
    async fn a_query_that_does_not_decode_is_400_said_without_its_value() {
        let refused = read("/posts?limit=sk_live_51").await.unwrap_err();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let body = refused
            .into_response()
            .into_body()
            .into_string()
            .await
            .unwrap();
        assert!(body.contains("a value its type does not accept"), "{body}");
        assert!(!body.contains("sk_live"), "{body}");
    }
}
