//! [`TestForm`]: a `multipart/form-data` body, field by field.

// The form's names and its encoding are ported from poem 3.1.12
// (`src/test/form.rs`, https://github.com/poem-web/poem), under its MIT
// license, whose notice follows:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

/// A `multipart/form-data` body (RFC 7578), sent with
/// [`TestRequestBuilder::multipart`](crate::TestRequestBuilder::multipart).
///
/// ```
/// use nest_rs_http::{Response, endpoint_fn};
/// use nest_rs_testing::{TestClient, TestForm, TestFormField};
///
/// # #[nest_rs_core::main]
/// # async fn main() {
/// # let client = TestClient::new(endpoint_fn(|_req| async { Ok(Response::default()) }));
/// let form = TestForm::new()
///     .text("title", "Hello")
///     .field(TestFormField::bytes(vec![7_u8; 16]).name("file").filename("a.bin"));
/// client.post("/uploads").multipart(form).send().await.assert_status_is_ok();
/// # }
/// ```
#[derive(Debug, Default)]
pub struct TestForm {
    fields: Vec<TestFormField>,
}

impl TestForm {
    /// The boundary between parts: no field this kit writes holds it.
    pub(crate) const BOUNDARY: &'static str = "__nestrs_multipart_boundary__";

    /// An empty form.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `field`.
    #[must_use]
    pub fn field(mut self, field: TestFormField) -> Self {
        self.fields.push(field);
        self
    }

    /// Adds a text field `name`.
    #[must_use]
    pub fn text(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.field(TestFormField::bytes(value.into()).name(name))
    }

    /// Adds a binary field `name`.
    #[must_use]
    pub fn bytes(self, name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        self.field(TestFormField::bytes(value).name(name))
    }

    /// The encoded body.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        let mut body = Vec::new();
        for field in self.fields {
            let mut disposition = String::from("form-data");
            if let Some(name) = field.name {
                disposition.push_str(&format!("; name=\"{}\"", quoted(&name)));
            }
            if let Some(filename) = field.filename {
                disposition.push_str(&format!("; filename=\"{}\"", quoted(&filename)));
            }
            body.extend_from_slice(format!("--{}\r\n", Self::BOUNDARY).as_bytes());
            body.extend_from_slice(
                format!("content-disposition: {disposition}\r\n\r\n").as_bytes(),
            );
            body.extend_from_slice(&field.data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{}--", Self::BOUNDARY).as_bytes());
        body
    }
}

/// One field of a [`TestForm`].
#[derive(Debug)]
pub struct TestFormField {
    name: Option<String>,
    filename: Option<String>,
    data: Vec<u8>,
}

impl TestFormField {
    /// A field holding `data`, with no name until given one.
    pub fn bytes(data: impl Into<Vec<u8>>) -> Self {
        Self {
            name: None,
            filename: None,
            data: data.into(),
        }
    }

    /// Names the field.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Gives the field a file name, as an upload carries.
    #[must_use]
    pub fn filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }
}

/// `text` as a quoted-string's content: a backslash, a quote and a line break
/// escaped, as poem wrote them.
fn quoted(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\\r")
        .replace('\n', "\\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_field_is_a_part_with_its_disposition() {
        let body = TestForm::new()
            .text("title", "Hello")
            .field(
                TestFormField::bytes(b"0123".to_vec())
                    .name("file")
                    .filename("a \"b\".txt"),
            )
            .into_bytes();
        let boundary = TestForm::BOUNDARY;
        assert_eq!(
            String::from_utf8(body).unwrap(),
            format!(
                "--{boundary}\r\ncontent-disposition: form-data; name=\"title\"\r\n\r\nHello\r\n\
                 --{boundary}\r\ncontent-disposition: form-data; name=\"file\"; \
                 filename=\"a \\\"b\\\".txt\"\r\n\r\n0123\r\n--{boundary}--"
            )
        );
    }
}
