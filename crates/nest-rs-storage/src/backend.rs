//! [`Backend`] — the store `<PREFIX>_STORAGE__URL` names: its scheme picks the
//! object_store backend, as a Redis URL's scheme picks its topology.

use object_store::path::Path;

/// The schemes this release opens, as a refusal lists them.
pub(crate) const OPENED: &str = "s3 and memory";

/// A store the URL names, parsed: the backend its scheme picks, and the prefix
/// every key is addressed under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// `s3://<bucket>[/<prefix>]`: AWS S3 or a server speaking its API.
    S3 {
        /// The bucket every key is addressed within.
        bucket: String,
        /// The keys' common prefix, empty for the bucket's root.
        prefix: Path,
    },
    /// `memory:///[<prefix>]`: a store in this process's memory, for
    /// development and tests.
    Memory {
        /// The keys' common prefix, empty for the store's root.
        prefix: Path,
    },
}

/// Why a URL names no store this release opens — a sentence that never quotes
/// the URL, whose userinfo may hold a credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unopened {
    /// The URL is blank.
    Blank,
    /// The URL does not parse.
    Unparsable,
    /// The scheme names a backend this release does not open.
    Scheme(String),
    /// An `s3://` URL naming no bucket.
    NoBucket,
    /// A part of the URL no backend reads: a credential, a port, a query.
    Extra(&'static str),
    /// A `memory://` URL naming a host.
    MemoryHost,
}

impl std::fmt::Display for Unopened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blank => f.write_str(
                "is not set — name the store: s3://<bucket>[/<prefix>], or memory:/// in a \
                 development or test profile",
            ),
            Self::Unparsable => f.write_str(
                "is not a URL — name the store: s3://<bucket>[/<prefix>], or memory:/// in a \
                 development or test profile",
            ),
            Self::Scheme(scheme) => write!(
                f,
                "names `{scheme}`, which this release does not open; it opens {OPENED}"
            ),
            Self::NoBucket => f.write_str("names no bucket — s3://<bucket>[/<prefix>]"),
            Self::Extra(part) => write!(
                f,
                "carries {part}, which no store reads — s3://<bucket>[/<prefix>], with the \
                 credentials in their own variables"
            ),
            Self::MemoryHost => {
                f.write_str("names a host after memory://, which takes none — memory:///")
            }
        }
    }
}

impl Backend {
    /// The store `url` names, or why it names none this release opens.
    pub(crate) fn parse(url: &str) -> Result<Self, Unopened> {
        let url = url.trim();
        if url.is_empty() {
            return Err(Unopened::Blank);
        }
        #[expect(
            clippy::map_err_ignore,
            reason = "the parser's error may quote the URL, whose userinfo may hold a credential"
        )]
        let parsed = url::Url::parse(url).map_err(|_| Unopened::Unparsable)?;
        if parsed.cannot_be_a_base() {
            return Err(Unopened::Unparsable);
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(Unopened::Extra("a credential"));
        }
        if parsed.port().is_some() {
            return Err(Unopened::Extra("a port"));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(Unopened::Extra("a query or a fragment"));
        }
        let prefix = Path::from(parsed.path().trim_matches('/'));
        let host = parsed.host_str().unwrap_or_default();
        match parsed.scheme() {
            "s3" if host.trim().is_empty() => Err(Unopened::NoBucket),
            "s3" => Ok(Self::S3 {
                bucket: host.to_owned(),
                prefix,
            }),
            "memory" if !host.is_empty() => Err(Unopened::MemoryHost),
            "memory" => Ok(Self::Memory { prefix }),
            other => Err(Unopened::Scheme(other.to_owned())),
        }
    }

    /// The scheme naming this backend, as every sentence about it names it.
    pub(crate) fn scheme(&self) -> &'static str {
        match self {
            Self::S3 { .. } => "s3",
            Self::Memory { .. } => "memory",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_s3_url_names_its_bucket_and_its_prefix() {
        assert_eq!(
            Backend::parse("s3://bucket/a/b"),
            Ok(Backend::S3 {
                bucket: "bucket".into(),
                prefix: Path::from("a/b"),
            })
        );
        assert_eq!(
            Backend::parse(" s3://media/ "),
            Ok(Backend::S3 {
                bucket: "media".into(),
                prefix: Path::default(),
            })
        );
    }

    #[test]
    fn a_memory_url_opens_the_process_store() {
        for url in ["memory:///", "memory://"] {
            assert_eq!(
                Backend::parse(url),
                Ok(Backend::Memory {
                    prefix: Path::default()
                }),
                "{url}"
            );
        }
        assert_eq!(
            Backend::parse("memory:///tenants/acme"),
            Ok(Backend::Memory {
                prefix: Path::from("tenants/acme")
            })
        );
        assert_eq!(Backend::parse("memory://host/"), Err(Unopened::MemoryHost));
    }

    #[test]
    fn a_scheme_this_release_does_not_open_is_named_and_the_url_never_quoted() {
        for (url, scheme) in [
            ("file:///var/lib/media", "file"),
            ("gs://bucket", "gs"),
            ("az://container", "az"),
            ("https://s3cr3t.example/bucket", "https"),
        ] {
            let refused = Backend::parse(url).expect_err("not opened").to_string();
            assert!(
                refused.contains(&format!("`{scheme}`")) && refused.contains("s3 and memory"),
                "{refused}"
            );
            assert!(!refused.contains(url), "{refused}");
            assert!(!refused.contains("s3cr3t"), "{refused}");
        }
    }

    #[test]
    fn a_url_carrying_what_no_store_reads_is_refused_without_quoting_it() {
        for (url, part) in [
            ("s3://key:s3cr3t@bucket/a", "a credential"),
            ("s3://bucket:9000", "a port"),
            ("s3://bucket/a?token=s3cr3t", "a query"),
        ] {
            let refused = Backend::parse(url).expect_err("refused").to_string();
            assert!(refused.contains(part), "{refused}");
            assert!(!refused.contains("s3cr3t"), "{refused}");
        }
        assert_eq!(Backend::parse("s3:///a"), Err(Unopened::NoBucket));
        assert_eq!(Backend::parse("  "), Err(Unopened::Blank));
        assert_eq!(Backend::parse("not a url"), Err(Unopened::Unparsable));
    }
}
