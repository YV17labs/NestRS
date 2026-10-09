//! [`Asset`] — a file found under the root, ready to answer with — and
//! [`Found`], every outcome of looking one up.

use bytes::Bytes;
use poem::http::HeaderValue;

use crate::validators::Validators;

pub(crate) enum AssetBody {
    /// An embedded file, or a small one read whole while it was looked up.
    Bytes(Bytes),
    /// A file too large to hold, streamed from where the range starts.
    File(std::fs::File),
}

pub(crate) struct Asset {
    pub(crate) body: AssetBody,
    pub(crate) len: u64,
    pub(crate) content_type: HeaderValue,
    /// An HTML document is revalidated whatever lifetime the other files get.
    pub(crate) html: bool,
    /// `None` for a file whose modification time the system does not report.
    pub(crate) validators: Option<Validators>,
}

pub(crate) enum Found {
    /// A file, or a directory's index.
    Asset(Asset),
    /// The root's index, answering a navigation no file matched.
    Fallback(Asset),
    Missing,
    /// A link under the root resolves outside it, or onto a hidden name.
    Escaped,
}
