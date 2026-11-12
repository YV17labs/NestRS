//! [`ManifestEdit`]: an idempotent edit of a `Cargo.toml`, applied to the
//! document it parses to, so a manifest it cannot read refuses the run rather
//! than reading as one the edit already holds.

use toml_edit::{DocumentMut, TomlError};

/// What a manifest edit does to the parsed document: `Ok(true)` when it
/// changed it, `Ok(false)` when it held the edit already, `Err` naming what it
/// cannot extend.
type Apply = dyn Fn(&mut DocumentMut) -> Result<bool, String> + Send + Sync;

/// An idempotent edit of a `Cargo.toml`.
pub(crate) struct ManifestEdit(Box<Apply>);

impl ManifestEdit {
    /// The edit `apply` makes to the document.
    pub(crate) fn new(
        apply: impl Fn(&mut DocumentMut) -> Result<bool, String> + Send + Sync + 'static,
    ) -> Self {
        Self(Box::new(apply))
    }

    /// `content` edited, `None` when it holds the edit already; `Err` saying
    /// where it does not parse as TOML, or what the edit cannot extend.
    pub(crate) fn apply(&self, content: &str) -> Result<Option<String>, String> {
        let mut document = content
            .parse::<DocumentMut>()
            .map_err(|refused| located(content, &refused))?;
        Ok((self.0)(&mut document)?.then(|| document.to_string()))
    }
}

/// Where `content` stopped parsing and why, by line and column as a compiler
/// says it, without quoting the file.
fn located(content: &str, refused: &TomlError) -> String {
    let Some(span) = refused.span() else {
        return format!("it does not parse as TOML: {}", refused.message());
    };
    let before = content.get(..span.start).unwrap_or(content);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |last| last.chars().count())
        + 1;
    format!(
        "line {line}, column {column}: it does not parse as TOML: {}",
        refused.message()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_key() -> ManifestEdit {
        ManifestEdit::new(|document| {
            if document.contains_key("added") {
                return Ok(false);
            }
            document["added"] = toml_edit::value(true);
            Ok(true)
        })
    }

    #[test]
    fn an_edit_writes_once() {
        let edited = add_key()
            .apply("[package]\nname = \"x\"\n")
            .expect("parses")
            .expect("changed");
        assert!(edited.contains("added = true"), "{edited}");
        assert_eq!(add_key().apply(&edited), Ok(None));
    }

    #[test]
    fn a_manifest_that_does_not_parse_is_refused_where_it_stops_quoting_nothing() {
        let refused = add_key()
            .apply("[package]\nname = \"x\"\n[broken-secret\n")
            .expect_err("does not parse");
        assert!(refused.starts_with("line 3, column "), "{refused}");
        assert!(!refused.contains("broken-secret"), "{refused}");
    }
}
