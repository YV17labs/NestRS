//! [`Form`]: an `application/x-www-form-urlencoded` body read into a type.

use std::ops::{Deref, DerefMut};

use http::Method;
use serde::de::DeserializeOwned;

use crate::error::{FormError, Result};
use crate::poem_adapter::PoemAdapter;
use crate::request::{FromRequest, Request, RequestBody};

/// An HTML form, decoded into `T`: the body of a request whose
/// `Content-Type` is `application/x-www-form-urlencoded`, read under the
/// transport's cap ([`current_body_limit`](crate::current_body_limit)); on
/// `GET`, which has no body, the query string.
///
/// A body without a form's `Content-Type` answers `415`, a body past the cap
/// `413`, and a form that does not decode into `T` `400`, said without the
/// value it held.
///
/// ```
/// use nest_rs_http::{Form, FromRequest, HeaderValue, Method, Request, header};
///
/// #[derive(serde::Deserialize)]
/// struct Login {
///     email: String,
/// }
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
///
/// let (request, mut body) = Request::builder()
///     .method(Method::POST)
///     .header(
///         header::CONTENT_TYPE,
///         HeaderValue::from_static("application/x-www-form-urlencoded"),
///     )
///     .body("email=ada%40example.com")
///     .split();
/// let Form(login) = Form::<Login>::from_request(&request, &mut body).await?;
/// assert_eq!(login.email, "ada@example.com");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Form<T>(pub T);

impl<T> Form<T> {
    /// The value, out of its wrapper.
    ///
    /// ```
    /// use nest_rs_http::Form;
    ///
    /// assert_eq!(Form(7).into_inner(), 7);
    /// ```
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Form<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Form<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<'a, T: DeserializeOwned> FromRequest<'a> for Form<T> {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        if req.method() == Method::GET {
            return PoemAdapter::query(req.uri())
                .map(Form)
                .map_err(|refused| FormError::Malformed(refused).into());
        }
        let is_form = req
            .content_type()
            .and_then(crate::request::essence)
            .is_some_and(|essence| essence.is("application", "x-www-form-urlencoded"));
        if !is_form {
            return Err(FormError::MediaType.into());
        }
        let bytes = body
            .take()?
            .into_bytes_limit(crate::current_body_limit())
            .await?;
        PoemAdapter::form(bytes).await.map(Form)
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderValue, StatusCode, Uri, header};
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Login {
        email: String,
        remember: bool,
    }

    async fn post(content_type: Option<&'static str>, body: &'static str) -> Result<Form<Login>> {
        let mut builder = Request::builder().method(Method::POST);
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        }
        let (request, mut body) = builder.body(body).split();
        Form::<Login>::from_request(&request, &mut body).await
    }

    #[tokio::test]
    async fn a_posted_form_decodes_into_its_type() {
        let Form(login) = post(
            Some("application/x-www-form-urlencoded; charset=utf-8"),
            "email=ada%40example.com&remember=true",
        )
        .await
        .unwrap();
        assert_eq!(
            login,
            Login {
                email: "ada@example.com".to_owned(),
                remember: true,
            }
        );
    }

    #[tokio::test]
    async fn a_get_reads_its_form_from_the_query() {
        let (request, mut body) = Request::builder()
            .uri(Uri::from_static(
                "/login?email=ada%40example.com&remember=false",
            ))
            .finish()
            .split();
        let Form(login) = Form::<Login>::from_request(&request, &mut body)
            .await
            .unwrap();
        assert!(!login.remember);
    }

    #[tokio::test]
    async fn a_body_that_is_not_a_form_by_its_type_is_415_and_its_type_is_never_quoted() {
        for content_type in [None, Some("text/plain; secret=sk_live_51")] {
            let refused = post(content_type, "email=a&remember=true")
                .await
                .unwrap_err();
            assert_eq!(refused.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
            let body = refused
                .into_response()
                .into_body()
                .into_string()
                .await
                .unwrap();
            assert!(body.contains("application/x-www-form-urlencoded"), "{body}");
            assert!(!body.contains("sk_live"), "{body}");
        }
    }

    #[tokio::test]
    async fn a_form_that_does_not_decode_is_400_said_without_its_value() {
        let refused = post(
            Some("application/x-www-form-urlencoded"),
            "email=a&remember=sk_live_51",
        )
        .await
        .unwrap_err();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let body = refused
            .into_response()
            .into_body()
            .into_string()
            .await
            .unwrap();
        assert!(body.contains("url decode"), "{body}");
        assert!(!body.contains("sk_live"), "{body}");
    }

    #[tokio::test]
    async fn a_body_past_the_configured_cap_is_413() {
        let (request, mut body) = Request::builder()
            .method(Method::POST)
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            )
            .body("email=ada%40example.com&remember=true")
            .split();
        let refused =
            crate::raw_body::with_body_limit(4, Form::<Login>::from_request(&request, &mut body))
                .await
                .unwrap_err();
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
