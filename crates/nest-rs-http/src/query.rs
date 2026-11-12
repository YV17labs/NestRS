//! [`Query`]: the request's query string read into a type.

use std::ops::{Deref, DerefMut};

use nest_rs_core::DecodeError;
use serde::de::DeserializeOwned;

use crate::error::{QueryError, Result};
use crate::request::{FromRequest, Request, RequestBody};

/// The request's query string, decoded as `application/x-www-form-urlencoded`
/// into `T`; an absent query reads as an empty one.
///
/// A query that does not decode into `T` answers `400` naming the parameter
/// and what its type expected, never the value it held.
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
        let query = req.uri().query().unwrap_or_default();
        serde_urlencoded::from_str(query)
            .map(Query)
            .map_err(|refused| {
                QueryError(urlencoded_refusal::<T>(&refused, query.as_bytes())).into()
            })
    }
}

/// `refused`, the failure of `T`'s decode from the urlencoded `input`, located
/// in `T`.
pub(crate) fn urlencoded_refusal<T: DeserializeOwned>(
    refused: &serde_urlencoded::de::Error,
    input: &[u8],
) -> DecodeError {
    DecodeError::from(refused).located::<T, _>(serde_urlencoded::Deserializer::new(
        form_urlencoded::parse(input),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use http::Uri;
    use serde::Deserialize;

    use super::*;
    use crate::testing::refusal_detail;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Page {
        limit: u32,
        after: Option<String>,
    }

    async fn read<T: DeserializeOwned>(uri: &'static str) -> Result<Query<T>> {
        let (request, mut body) = Request::builder()
            .uri(Uri::from_static(uri))
            .finish()
            .split();
        Query::<T>::from_request(&request, &mut body).await
    }

    #[tokio::test]
    async fn a_query_decodes_percent_and_plus_into_its_type() {
        let Query(page) = read::<Page>("/posts?limit=5&after=a%2Fb+c").await.unwrap();
        assert_eq!(
            page,
            Page {
                limit: 5,
                after: Some("a/b c".to_owned()),
            }
        );
    }

    #[tokio::test]
    async fn a_query_that_does_not_decode_names_its_parameter_and_never_its_value() {
        let refused = read::<Page>("/posts?limit=sk_live_51").await.unwrap_err();
        assert_eq!(
            refusal_detail(refused).await,
            "query parameter `limit`: a value its type does not accept, expected u32"
        );

        #[derive(Debug, Deserialize)]
        #[expect(
            dead_code,
            reason = "the field exists for serde to read; the test asserts the error, never a value"
        )]
        struct Download {
            file: uuid::Uuid,
        }
        let refused = read::<Download>("/audio/download?file=not-a-uuid")
            .await
            .unwrap_err();
        let detail = refusal_detail(refused).await;
        assert!(detail.starts_with("query parameter `file`: "), "{detail}");
        assert!(!detail.contains("not-a-uuid"), "{detail}");
    }

    #[tokio::test]
    async fn a_missing_parameter_is_named_once() {
        let refused = read::<Page>("/posts?after=x").await.unwrap_err();
        assert_eq!(
            refusal_detail(refused).await,
            "query parameter `limit`: missing"
        );
    }

    #[tokio::test]
    async fn a_parameter_a_map_reads_is_said_as_an_entry_never_by_its_name() {
        let refused = read::<HashMap<String, u32>>("/posts?sk_live_51=seven")
            .await
            .unwrap_err();
        assert_eq!(
            refusal_detail(refused).await,
            "query parameter `[…]`: a value its type does not accept, expected u32"
        );
    }
}
