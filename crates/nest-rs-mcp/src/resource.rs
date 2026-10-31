//! What a `#[resource]` read and a `#[resources]` listing answer, read by type.

use std::marker::PhantomData;

use nest_rs_core::Failure;
use rmcp::handler::server::wrapper::Json;
use rmcp::model::{ListResourcesResult, ReadResourceResult, Resource, ResourceContents};
use serde::Serialize;

use crate::error::{OperationAnswer, OperationFailure, resource_not_found};

/// The media type a `Json<T>` resource is read as.
const JSON: &str = "application/json";

/// The media type a text resource is read as, rmcp's own for a text content.
const TEXT: &str = "text/plain";

/// What a `#[resource]` read answers, written into the contents of the URI the
/// client read.
#[diagnostic::on_unimplemented(
    message = "a `#[resource]` answers `Json<T>`, a `String` or a `ReadResourceResult`, and \
               `{Self}` is none of them",
    label = "not a resource's contents",
    note = "a `#[resource]` answers its contents as `Json<T>`, a `String` or a `ReadResourceResult`, or any of them wrapped in `Option` to answer a resource the caller may not read as not found"
)]
pub trait ResourceBody: Sized {
    /// The media type its contents carry, advertised on its template; `None`
    /// when only the value knows it.
    const MIME_TYPE: Option<&'static str>;

    /// The result reading `uri` answers.
    fn into_read(self, uri: String) -> Result<ReadResourceResult, OperationFailure>;
}

/// The value, serialized: a JSON text the client parses.
#[diagnostic::do_not_recommend]
impl<T: Serialize> ResourceBody for Json<T> {
    const MIME_TYPE: Option<&'static str> = Some(JSON);

    fn into_read(self, uri: String) -> Result<ReadResourceResult, OperationFailure> {
        let text = serde_json::to_string(&self.0)
            .map_err(|refused| OperationFailure::unanswered(Failure::new(refused)))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type(JSON),
        ]))
    }
}

/// Plain text.
#[diagnostic::do_not_recommend]
impl ResourceBody for String {
    const MIME_TYPE: Option<&'static str> = Some(TEXT);

    fn into_read(self, uri: String) -> Result<ReadResourceResult, OperationFailure> {
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            self, uri,
        )]))
    }
}

/// The protocol's own result, as built: a blob, or several contents.
#[diagnostic::do_not_recommend]
impl ResourceBody for ReadResourceResult {
    const MIME_TYPE: Option<&'static str> = None;

    fn into_read(self, _uri: String) -> Result<ReadResourceResult, OperationFailure> {
        Ok(self)
    }
}

/// `None` is a resource the caller may not read, answered as one no template
/// reads: a reader cannot tell a row it was refused from a row that is not.
#[diagnostic::do_not_recommend]
impl<B: ResourceBody> ResourceBody for Option<B> {
    const MIME_TYPE: Option<&'static str> = B::MIME_TYPE;

    fn into_read(self, uri: String) -> Result<ReadResourceResult, OperationFailure> {
        match self {
            Some(body) => body.into_read(uri),
            None => Err(OperationFailure::from(resource_not_found())),
        }
    }
}

/// Asks the answer `R` of a `#[resource]` method the media type its value is
/// read as, called on `&&MimeProbe<R>`: an answer that is no resource's body
/// reads none, so the one refusal of it is the read's own.
pub struct MimeProbe<R>(PhantomData<fn() -> R>);

impl<R> MimeProbe<R> {
    /// The probe of `R`.
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<R> Default for MimeProbe<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// The media type of an answer whose value is a resource's body.
pub trait MimeProbeReads {
    /// Its value's [`ResourceBody::MIME_TYPE`].
    fn mime_type(&self) -> Option<&'static str>;
}

impl<R> MimeProbeReads for &MimeProbe<R>
where
    R: OperationAnswer,
    R::Value: ResourceBody,
{
    fn mime_type(&self) -> Option<&'static str> {
        <R::Value as ResourceBody>::MIME_TYPE
    }
}

/// Any other answer, which the read refuses: none.
pub trait MimeProbeFallback {
    /// No media type.
    fn mime_type(&self) -> Option<&'static str> {
        None
    }
}

impl<R> MimeProbeFallback for MimeProbe<R> {}

/// What a `#[resources]` listing answers.
#[diagnostic::on_unimplemented(
    message = "a `#[resources]` listing answers `Vec<Resource>`, and `{Self}` is not one",
    label = "not a resource listing"
)]
pub trait ResourceListing {
    /// The listing `resources/list` answers.
    fn into_listing(self) -> ListResourcesResult;
}

impl ResourceListing for Vec<Resource> {
    fn into_listing(self) -> ListResourcesResult {
        ListResourcesResult::with_all_items(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_carries_the_uri_it_was_read_at_and_its_media_type() {
        let read = Json(serde_json::json!({ "id": 1 }))
            .into_read("note://1".to_owned())
            .ok()
            .expect("a JSON value serializes");
        assert_eq!(
            serde_json::to_value(&read.contents).ok(),
            Some(serde_json::json!([{
                "uri": "note://1",
                "mimeType": "application/json",
                "text": r#"{"id":1}"#,
            }])),
        );
        assert_eq!(<Json<()> as ResourceBody>::MIME_TYPE, Some(JSON));
        assert_eq!(<Option<String> as ResourceBody>::MIME_TYPE, Some(TEXT));
        assert_eq!(<ReadResourceResult as ResourceBody>::MIME_TYPE, None);
    }

    /// The expansion's call, written once so every test asks at the depth
    /// `#[tools]` does.
    macro_rules! mime_type {
        ($answer:ty) => {
            (&&MimeProbe::<$answer>::new()).mime_type()
        };
    }

    /// A body's media type, and none for an answer the read itself refuses.
    #[test]
    fn the_probe_reads_a_body_s_media_type_and_nothing_else() {
        use super::{MimeProbeFallback as _, MimeProbeReads as _};

        type Answered<T> = Result<T, crate::McpError>;
        assert_eq!(mime_type!(Answered<Option<Json<u8>>>), Some(JSON));
        assert_eq!(mime_type!(Answered<String>), Some(TEXT));
        assert_eq!(mime_type!(Answered<u8>), None);
        assert_eq!(mime_type!(String), None);
    }

    #[test]
    fn none_is_answered_as_a_resource_no_template_reads() {
        let missing = None::<String>
            .into_read("note://2".to_owned())
            .expect_err("nothing to read")
            .into_answer();
        assert_eq!(missing, resource_not_found());
    }
}
