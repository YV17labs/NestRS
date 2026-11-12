//! What a statement does and to which table, read off its SQL text as the
//! database conventions summarise it (`SELECT posts`) — never a value it
//! carries: literals, comments and quoted text are skipped unread.

/// A statement's operation and the first table it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QuerySummary<'a> {
    operation: &'static str,
    collection: Option<&'a str>,
}

/// The operations a summary names; any other first word names none, so a
/// statement's text never becomes a series.
const OPERATIONS: &[&str] = &[
    "ALTER",
    "ANALYZE",
    "BEGIN",
    "CALL",
    "CLOSE",
    "COMMIT",
    "COPY",
    "CREATE",
    "DEALLOCATE",
    "DECLARE",
    "DELETE",
    "DISCARD",
    "DO",
    "DROP",
    "EXECUTE",
    "EXPLAIN",
    "FETCH",
    "GRANT",
    "INSERT",
    "LISTEN",
    "LOCK",
    "MERGE",
    "NOTIFY",
    "PREPARE",
    "REFRESH",
    "REINDEX",
    "RELEASE",
    "RESET",
    "REVOKE",
    "ROLLBACK",
    "SAVEPOINT",
    "SELECT",
    "SET",
    "SHOW",
    "START",
    "TRUNCATE",
    "UNLISTEN",
    "UPDATE",
    "VACUUM",
    "VALUES",
    "WITH",
];

impl<'a> QuerySummary<'a> {
    /// The summary of `sql`, or `None` when its first word is no operation
    /// the conventions' summaries name.
    pub(crate) fn of(sql: &'a str) -> Option<Self> {
        let mut words = Words::new(sql);
        let first = words.next()?;
        let mut operation = operation(first.text)?;
        if operation == "WITH" {
            // The statement a CTE leads to is the first operation outside its
            // parentheses.
            operation = words
                .by_ref()
                .filter(|word| word.depth == 0)
                .find_map(|word| operation_after_with(word.text))?;
        }
        let introducer = match operation {
            "SELECT" | "DELETE" => Some("FROM"),
            "INSERT" | "MERGE" => Some("INTO"),
            "UPDATE" => None,
            _ => {
                return Some(Self {
                    operation,
                    collection: None,
                });
            }
        };
        let collection = match introducer {
            None => words.next().and_then(|word| identifier(word.text)),
            Some(introducer) => words
                .by_ref()
                .find(|word| word.depth == 0 && word.text.eq_ignore_ascii_case(introducer))
                .and_then(|_| words.next())
                .and_then(|word| identifier(word.text)),
        };
        Some(Self {
            operation,
            collection,
        })
    }

    /// The operation, uppercase (`SELECT`).
    pub(crate) const fn operation(&self) -> &'static str {
        self.operation
    }

    /// The first table the statement names, as written, unquoted.
    pub(crate) const fn collection(&self) -> Option<&'a str> {
        self.collection
    }

    /// The summary itself: the operation and the table, or the operation alone.
    pub(crate) fn text(&self) -> String {
        match self.collection {
            Some(collection) => format!("{} {collection}", self.operation),
            None => self.operation.to_owned(),
        }
    }
}

/// `word` as one of [`OPERATIONS`].
fn operation(word: &str) -> Option<&'static str> {
    OPERATIONS
        .iter()
        .find(|operation| operation.eq_ignore_ascii_case(word))
        .copied()
}

/// The statement a `WITH` leads to, where `word` is one.
fn operation_after_with(word: &str) -> Option<&'static str> {
    ["SELECT", "INSERT", "UPDATE", "DELETE", "MERGE"]
        .into_iter()
        .find(|operation| operation.eq_ignore_ascii_case(word))
}

/// `word` as a table name: a bare identifier, a quoted one without its quotes,
/// or a qualified one as written; a keyword or punctuation names none.
fn identifier(word: &str) -> Option<&str> {
    let unquoted = word
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .filter(|inner| !inner.contains('"'));
    match unquoted {
        Some(inner) if !inner.is_empty() => Some(inner),
        _ if word.starts_with(|c: char| c == '"' || c.is_ascii_alphabetic() || c == '_')
            && operation(word).is_none()
            && !word.eq_ignore_ascii_case("ONLY") =>
        {
            Some(word)
        }
        _ => None,
    }
}

/// One word of a statement and the depth of parentheses it sits at.
#[derive(Clone, Copy, Debug)]
struct Word<'a> {
    text: &'a str,
    depth: usize,
}

/// The words of a statement — identifiers, quoted or qualified, and keywords —
/// skipping its literals, comments and punctuation.
struct Words<'a> {
    sql: &'a str,
    at: usize,
    depth: usize,
}

impl<'a> Words<'a> {
    const fn new(sql: &'a str) -> Self {
        Self {
            sql,
            at: 0,
            depth: 0,
        }
    }

    /// Past the byte at `self.at` up to and including `close`, or to the end.
    fn skip_past(&mut self, close: &str) {
        self.at = self.sql[self.at..]
            .find(close)
            .map_or(self.sql.len(), |found| self.at + found + close.len());
    }
}

impl<'a> Iterator for Words<'a> {
    type Item = Word<'a>;

    fn next(&mut self) -> Option<Word<'a>> {
        let bytes = self.sql.as_bytes();
        while self.at < bytes.len() {
            let rest = &self.sql[self.at..];
            match bytes[self.at] {
                b'(' => {
                    self.depth += 1;
                    self.at += 1;
                }
                b')' => {
                    self.depth = self.depth.saturating_sub(1);
                    self.at += 1;
                }
                b'\'' => {
                    self.at += 1;
                    self.skip_past("'");
                }
                b'$' if rest.starts_with("$$") => {
                    self.at += 2;
                    self.skip_past("$$");
                }
                b'-' if rest.starts_with("--") => self.skip_past("\n"),
                b'/' if rest.starts_with("/*") => self.skip_past("*/"),
                byte if byte == b'"' || byte.is_ascii_alphabetic() || byte == b'_' => {
                    let start = self.at;
                    loop {
                        match bytes.get(self.at) {
                            Some(b'"') => {
                                self.at += 1;
                                self.skip_past("\"");
                            }
                            Some(byte) if byte.is_ascii_alphanumeric() || *byte == b'_' => {
                                self.at += 1;
                            }
                            Some(b'.')
                                if bytes.get(self.at + 1).is_some_and(|next| {
                                    *next == b'"' || next.is_ascii_alphabetic() || *next == b'_'
                                }) =>
                            {
                                self.at += 1;
                            }
                            _ => break,
                        }
                    }
                    return Some(Word {
                        text: &self.sql[start..self.at],
                        depth: self.depth,
                    });
                }
                _ => self.at += 1,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(sql: &str) -> Option<(&'static str, Option<&str>)> {
        QuerySummary::of(sql).map(|summary| (summary.operation(), summary.collection()))
    }

    #[test]
    fn what_sea_query_writes_is_summarised_by_its_operation_and_its_table() {
        let cases = [
            (
                r#"SELECT "posts"."id", "posts"."title" FROM "posts" WHERE "posts"."id" = $1 LIMIT $2"#,
                ("SELECT", Some("posts")),
            ),
            (
                r#"INSERT INTO "posts" ("id", "title") VALUES ($1, $2) RETURNING "id""#,
                ("INSERT", Some("posts")),
            ),
            (
                r#"UPDATE "posts" SET "title" = $1 WHERE "posts"."id" = $2"#,
                ("UPDATE", Some("posts")),
            ),
            (
                r#"DELETE FROM "posts" WHERE "posts"."id" = $1"#,
                ("DELETE", Some("posts")),
            ),
            (
                r#"SELECT COUNT(*) AS "num_items" FROM (SELECT "posts"."id" FROM "posts") AS "sub_query""#,
                ("SELECT", None),
            ),
            ("BEGIN", ("BEGIN", None)),
            ("SAVEPOINT sp1", ("SAVEPOINT", None)),
        ];
        for (sql, expected) in cases {
            assert_eq!(summary(sql), Some(expected), "{sql}");
        }
    }

    #[test]
    fn a_hand_written_statement_is_summarised_whatever_its_case_and_qualification() {
        assert_eq!(
            summary("select id from public.posts where title = 'FROM users'"),
            Some(("SELECT", Some("public.posts"))),
        );
        assert_eq!(
            summary("WITH recent AS (SELECT id FROM posts) DELETE FROM drafts USING recent"),
            Some(("DELETE", Some("drafts"))),
        );
        assert_eq!(
            summary("-- a comment naming FROM secrets\nSELECT 1"),
            Some(("SELECT", None)),
        );
    }

    #[test]
    fn a_literal_a_comment_or_a_dollar_quoted_body_never_reaches_the_summary() {
        for sql in [
            "SELECT * FROM 'secret_table'",
            "SELECT /* FROM hidden */ 1",
            "DO $$ BEGIN DELETE FROM secret; END $$",
        ] {
            let summary = QuerySummary::of(sql).expect("a summary");
            assert!(
                !summary.text().contains("secret") && !summary.text().contains("hidden"),
                "{sql}: {}",
                summary.text(),
            );
        }
    }

    #[test]
    fn a_first_word_no_summary_names_names_no_operation() {
        assert_eq!(summary("frobnicate the database"), None);
        assert_eq!(summary(""), None);
        assert_eq!(summary("'only a literal'"), None);
    }

    #[test]
    fn the_summary_reads_the_operation_and_its_table() {
        let summary = QuerySummary::of(r#"SELECT "id" FROM "posts""#).expect("a summary");
        assert_eq!(summary.text(), "SELECT posts");
        let summary = QuerySummary::of("COMMIT").expect("a summary");
        assert_eq!(summary.text(), "COMMIT");
    }
}
