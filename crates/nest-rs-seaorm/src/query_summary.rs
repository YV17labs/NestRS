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
    /// the conventions' summaries name, or when its words cannot be read with
    /// certainty up to the table it names: a guessed word could be a value.
    pub(crate) fn of(sql: &'a str) -> Option<Self> {
        let mut words = Words::new(sql);
        let summary = Self::read(&mut words);
        if words.unreadable { None } else { summary }
    }

    fn read(words: &mut Words<'a>) -> Option<Self> {
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
        let introduced = match introducer {
            None => true,
            Some(introducer) => {
                // `IS [NOT] DISTINCT FROM` compares; its `FROM` names no table.
                let mut previous: Option<&str> = None;
                words.by_ref().any(|word| {
                    let introduces = word.depth == 0
                        && word.text.eq_ignore_ascii_case(introducer)
                        && !previous.is_some_and(|word| word.eq_ignore_ascii_case("DISTINCT"));
                    previous = Some(word.text);
                    introduces
                })
            }
        };
        let collection = introduced
            .then(|| words.next())
            .flatten()
            .filter(|word| !word.follows_gap)
            .and_then(|word| identifier(word.text));
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
        _ if word
            .starts_with(|c: char| c == '"' || c == '_' || c.is_alphabetic() || !c.is_ascii())
            && operation(word).is_none()
            && !word.eq_ignore_ascii_case("ONLY") =>
        {
            Some(word)
        }
        _ => None,
    }
}

/// One word of a statement, the depth of parentheses it sits at, and whether
/// anything but blanks and comments sits between it and the word before.
#[derive(Clone, Copy, Debug)]
struct Word<'a> {
    text: &'a str,
    depth: usize,
    follows_gap: bool,
}

/// How a quoted string reads a backslash.
#[derive(Clone, Copy)]
enum Backslash {
    /// An escape string's (`E'…'`): it escapes the character after it.
    Escapes,
    /// A Unicode string's (`U&'…'`): a character of its own.
    Literal,
    /// A standard string's: a character of its own or an escape, as the
    /// server's `standard_conforming_strings` says, which the text does not.
    Unknown,
}

/// The words of a statement — identifiers, quoted or qualified, and keywords —
/// skipping its literals, comments and punctuation as Postgres's lexer does.
/// What it cannot read with certainty — a quote or a comment never closed, a
/// backslash in a standard string — ends the words there and marks the
/// statement [`unreadable`](Self::unreadable).
struct Words<'a> {
    sql: &'a str,
    at: usize,
    depth: usize,
    unreadable: bool,
    /// Something but blanks and comments was skipped since the last word.
    gap: bool,
}

impl<'a> Words<'a> {
    const fn new(sql: &'a str) -> Self {
        Self {
            sql,
            at: 0,
            depth: 0,
            unreadable: false,
            gap: false,
        }
    }

    fn byte(&self, at: usize) -> Option<u8> {
        self.sql.as_bytes().get(at).copied()
    }

    /// Past the quoted text whose body starts at `from` and ends at `quote`,
    /// doubled to stand for itself; `false` where nothing closes it, or where a
    /// backslash may escape the quote.
    fn quoted(&mut self, from: usize, quote: u8, backslash: Backslash) -> bool {
        let mut at = from;
        while let Some(byte) = self.byte(at) {
            match byte {
                byte if byte == quote => {
                    if self.byte(at + 1) == Some(quote) {
                        at += 2;
                    } else {
                        self.at = at + 1;
                        return true;
                    }
                }
                b'\\' => match backslash {
                    Backslash::Escapes => at += 2,
                    Backslash::Literal => at += 1,
                    Backslash::Unknown => return false,
                },
                _ => at += 1,
            }
        }
        false
    }

    /// Past the block comment whose body starts at `from`, comments nested in
    /// it included; `false` where it never closes.
    fn comment(&mut self, from: usize) -> bool {
        let mut at = from;
        let mut open = 1_usize;
        while let Some(byte) = self.byte(at) {
            match (byte, self.byte(at + 1)) {
                (b'/', Some(b'*')) => {
                    open += 1;
                    at += 2;
                }
                (b'*', Some(b'/')) => {
                    open -= 1;
                    at += 2;
                    if open == 0 {
                        self.at = at;
                        return true;
                    }
                }
                _ => at += 1,
            }
        }
        false
    }

    /// Past what a `$` at `self.at` opens: a parameter (`$1`), or a body quoted
    /// between two `$tag$`; `false` where neither reads.
    fn dollar(&mut self) -> bool {
        let start = self.at;
        if self
            .byte(start + 1)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.at = start + 1;
            while self.byte(self.at).is_some_and(|byte| byte.is_ascii_digit()) {
                self.at += 1;
            }
            return true;
        }
        let mut end = start + 1;
        if self.byte(end).is_some_and(starts_identifier) {
            while self
                .byte(end)
                .is_some_and(|byte| continues_identifier(byte) && byte != b'$')
            {
                end += 1;
            }
        }
        if self.byte(end) != Some(b'$') {
            return false;
        }
        let tag = &self.sql.as_bytes()[start..=end];
        let body = end + 1;
        match self.sql.as_bytes()[body..]
            .windows(tag.len())
            .position(|window| window == tag)
        {
            Some(found) => {
                self.at = body + found + tag.len();
                true
            }
            None => false,
        }
    }

    /// The word starting at `self.at`, or past the literal its first letter
    /// opens (`E'…'`, `B'…'`, `X'…'`, `N'…'`, `U&'…'`, `U&"…"`).
    fn word(&mut self) -> Result<Option<Word<'a>>, ()> {
        let start = self.at;
        let first = self.byte(start).unwrap_or_default();
        match (
            first.to_ascii_uppercase(),
            self.byte(start + 1),
            self.byte(start + 2),
        ) {
            (b'E', Some(b'\''), _) => {
                return self
                    .quoted(start + 2, b'\'', Backslash::Escapes)
                    .then_some(None)
                    .ok_or(());
            }
            (b'B' | b'X' | b'N', Some(b'\''), _) => {
                return self
                    .quoted(start + 2, b'\'', Backslash::Unknown)
                    .then_some(None)
                    .ok_or(());
            }
            (b'U', Some(b'&'), Some(b'\'')) => {
                return self
                    .quoted(start + 3, b'\'', Backslash::Literal)
                    .then_some(None)
                    .ok_or(());
            }
            (b'U', Some(b'&'), Some(b'"')) => {
                return self
                    .quoted(start + 3, b'"', Backslash::Literal)
                    .then_some(None)
                    .ok_or(());
            }
            _ => {}
        }
        while let Some(byte) = self.byte(self.at) {
            match byte {
                b'"' => {
                    if !self.quoted(self.at + 1, b'"', Backslash::Literal) {
                        return Err(());
                    }
                }
                byte if continues_identifier(byte) => self.at += 1,
                b'.' if self
                    .byte(self.at + 1)
                    .is_some_and(|next| next == b'"' || starts_identifier(next)) =>
                {
                    self.at += 1;
                }
                _ => break,
            }
        }
        match self.sql.get(start..self.at) {
            Some(text) => Ok(Some(Word {
                text,
                depth: self.depth,
                follows_gap: std::mem::replace(&mut self.gap, false),
            })),
            None => Err(()),
        }
    }
}

/// Whether `byte` starts an identifier: a letter, `_`, or any byte of a
/// character outside ASCII, which Postgres reads as a letter.
const fn starts_identifier(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || !byte.is_ascii()
}

/// Whether `byte` continues an identifier: a digit and `$` besides.
const fn continues_identifier(byte: u8) -> bool {
    starts_identifier(byte) || byte.is_ascii_digit() || byte == b'$'
}

impl<'a> Iterator for Words<'a> {
    type Item = Word<'a>;

    fn next(&mut self) -> Option<Word<'a>> {
        while !self.unreadable {
            let byte = self.byte(self.at)?;
            let next = self.byte(self.at + 1);
            let blank = byte.is_ascii_whitespace()
                || (byte == b'-' && next == Some(b'-'))
                || (byte == b'/' && next == Some(b'*'));
            if !blank && byte != b'"' && !starts_identifier(byte) {
                self.gap = true;
            }
            let read = match byte {
                b'(' => {
                    self.depth += 1;
                    self.at += 1;
                    true
                }
                b')' => {
                    self.depth = self.depth.saturating_sub(1);
                    self.at += 1;
                    true
                }
                b'\'' => self.quoted(self.at + 1, b'\'', Backslash::Unknown),
                b'$' => self.dollar(),
                b'-' if next == Some(b'-') => {
                    self.at = self.sql[self.at..]
                        .find('\n')
                        .map_or(self.sql.len(), |found| self.at + found + 1);
                    true
                }
                b'/' if next == Some(b'*') => self.comment(self.at + 2),
                b'0'..=b'9' => {
                    while self.byte(self.at).is_some_and(|byte| {
                        byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.'
                    }) {
                        self.at += 1;
                    }
                    true
                }
                byte if byte == b'"' || starts_identifier(byte) => match self.word() {
                    Ok(Some(word)) => return Some(word),
                    // A literal a letter opens (`E'…'`, `U&"…"`).
                    Ok(None) => {
                        self.gap = true;
                        true
                    }
                    Err(()) => false,
                },
                _ => {
                    self.at += 1;
                    true
                }
            };
            if !read {
                self.unreadable = true;
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

    /// A table is the word right after its introducer, and a `FROM` closing
    /// `IS DISTINCT FROM` introduces none: anything else between them — a
    /// literal, a parameter, an identifier read as a literal is — leaves the
    /// table unnamed rather than names the word after it.
    #[test]
    fn a_table_is_named_only_by_the_word_its_introducer_leads_to() {
        let cases = [
            ("SELECT a IS DISTINCT FROM b FROM t", ("SELECT", Some("t"))),
            (
                "SELECT a IS NOT DISTINCT FROM $1 FROM t",
                ("SELECT", Some("t")),
            ),
            (r#"SELECT * FROM U&"t\0061" WHERE x = 1"#, ("SELECT", None)),
            (
                "SELECT * FROM /* the users */ users",
                ("SELECT", Some("users")),
            ),
            ("UPDATE (SELECT 1) SET x = 1", ("UPDATE", None)),
        ];
        for (sql, expected) in cases {
            assert_eq!(summary(sql), Some(expected), "{sql}");
        }
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
    fn a_word_outside_ascii_is_read_as_postgres_reads_it_and_never_splits_a_character() {
        assert_eq!(summary("SELECT naïve FROM t"), Some(("SELECT", Some("t"))));
        assert_eq!(
            summary("SELECT 1 FROM café"),
            Some(("SELECT", Some("café")))
        );
        assert_eq!(summary("SELECT ½ FROM t"), Some(("SELECT", Some("t"))));
        assert_eq!(summary("\u{feff}SELECT 1"), None);
        assert_eq!(
            summary("SELECT $q$Café$q$ FROM t"),
            Some(("SELECT", Some("t")))
        );
    }

    #[test]
    fn every_literal_and_comment_form_postgres_lexes_is_skipped_unread() {
        let cases = [
            (r"SELECT E'it\'s from alice' FROM t", ("SELECT", Some("t"))),
            (r"SELECT e'\\' FROM t", ("SELECT", Some("t"))),
            ("SELECT E'it''s from alice' FROM t", ("SELECT", Some("t"))),
            ("SELECT 'it''s from alice' FROM t", ("SELECT", Some("t"))),
            (
                "SELECT U&'d\\0061t from alice' FROM t",
                ("SELECT", Some("t")),
            ),
            (
                "SELECT B'0101', X'1F', N'from alice' FROM t",
                ("SELECT", Some("t")),
            ),
            (
                "SELECT $body$ it's from alice $body$ FROM t",
                ("SELECT", Some("t")),
            ),
            (
                "SELECT $a$ $b$ from alice $b$ $a$ FROM t",
                ("SELECT", Some("t")),
            ),
            (
                "SELECT /* a /* from alice */ still from alice */ 1 FROM t",
                ("SELECT", Some("t")),
            ),
            (r#"SELECT "from ""alice""" FROM t"#, ("SELECT", Some("t"))),
            ("SELECT $1, $2 FROM t WHERE a = $3", ("SELECT", Some("t"))),
            ("SELECT price$ FROM t", ("SELECT", Some("t"))),
            ("DO $fn$ BEGIN DELETE FROM alice; END $fn$", ("DO", None)),
        ];
        for (sql, expected) in cases {
            assert_eq!(summary(sql), Some(expected), "{sql}");
        }
    }

    #[test]
    fn a_statement_whose_words_cannot_be_read_with_certainty_has_no_summary() {
        for sql in [
            // A backslash in a standard string ends it or escapes its quote,
            // as the server's `standard_conforming_strings` says.
            r"SELECT 'it\' FROM alice' FROM t",
            "SELECT 'never closed FROM alice",
            "SELECT E'never closed FROM alice",
            r#"SELECT "never closed FROM alice"#,
            "SELECT /* never closed FROM alice",
            "SELECT /* a /* b */ FROM alice",
            "SELECT $tag$ never closed FROM alice",
            "SELECT $not a tag FROM alice",
        ] {
            assert_eq!(summary(sql), None, "{sql}");
        }
    }

    #[test]
    fn no_statement_cut_anywhere_panics_or_names_what_it_quotes() {
        let sql = "WITH café AS (SELECT naïve FROM t) SELECT E'it\\'s ½', $x$ é $x$, \
                   /* a /* é */ b */ \"é\"\"x\", U&'\\00e9', $1 FROM \"wé\".tbl -- é\n;";
        for (at, _) in sql.char_indices().chain([(sql.len(), ' ')]) {
            if let Some(summary) = QuerySummary::of(&sql[..at]) {
                assert!(!summary.text().contains('½'), "{}", &sql[..at]);
            }
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
