use sea_orm::sea_query::{
    ArrayType, Condition, Expr, OptionEnum, Order, PostgresQueryBuilder, QueryBuilder,
    SqlWriterValues, Values,
};
use sea_orm::{ActiveModelTrait, DbErr, EntityTrait, Iterable, Value};

/// Why `Repo` refused a developer's condition or value: its text, as Postgres reads
/// it, would not stay inside the place `Repo` gives it in the statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Uncontained {
    /// A `)` closing what the condition did not open, or a `(` it leaves open.
    Parenthesis,
    /// A `'…'` or `E'…'` string it leaves open.
    String,
    /// A `"…"` identifier it leaves open.
    Identifier,
    /// A comment, `--` or `/* */`: a condition needs none, and one left open reads
    /// the rest of the statement as its text.
    Comment,
    /// Dollar quoting (`$$…$$`, `$tag$…$tag$`), or a `$` that is no placeholder.
    Dollar,
    /// A backslash in a `'…'` string, which escapes the next character where
    /// `standard_conforming_strings` is off: the string would end elsewhere.
    Backslash,
    /// A NUL byte, which ends the text Postgres reads.
    Nul,
    /// An enum value whose type name holds a `"`, written unescaped between quotes.
    TypeName,
}

impl Uncontained {
    /// The `reason` the refusal's `warn` names it by.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Parenthesis => "unbalanced_parenthesis",
            Self::String => "unclosed_string",
            Self::Identifier => "unclosed_identifier",
            Self::Comment => "comment",
            Self::Dollar => "dollar_outside_placeholder",
            Self::Backslash => "backslash_in_string",
            Self::Nul => "nul_byte",
            Self::TypeName => "quote_in_type_name",
        }
    }
}

/// `condition` as one parenthesized operand, so a balanced text holding an `OR` stays
/// beside the scope rather than reaching past it; `None` for an empty `all`, which is
/// `TRUE`. Refused when its text, its values left as placeholders, would not stay
/// inside that operand.
pub(crate) fn contained(condition: Condition) -> Result<Option<Expr>, Uncontained> {
    let expr = Expr::from(condition);
    if matches!(expr, Expr::Constant(Value::Bool(Some(true)))) {
        return Ok(None);
    }
    let mut text = SqlWriterValues::new("$", true);
    PostgresQueryBuilder.prepare_expr(&expr, &mut text);
    scan(text.into_parts().0.as_bytes())?;
    Ok(Some(Expr::tuple([expr])))
}

/// Refuses a value of `active` whose enum type name would leave its quotes: the one
/// text a bound value writes into the statement.
pub(crate) fn contained_values<A: ActiveModelTrait>(active: &A) -> Result<(), Uncontained> {
    <A::Entity as EntityTrait>::Column::iter().try_for_each(|column| {
        active
            .get(column)
            .into_value()
            .map_or(Ok(()), |value| contained_value(&value))
    })
}

/// Refuses an order by field whose values `contained_value` refuses: sea-query writes
/// them inline, in the statement's `ORDER BY`.
pub(crate) fn contained_order(order: &Order) -> Result<(), Uncontained> {
    match order {
        Order::Field(Values(values)) => values.iter().try_for_each(contained_value),
        _ => Ok(()),
    }
}

/// Refuses `value` when its enum type name holds a `"`, which closes the quotes
/// sea-query writes it between, as `$1::"<type_name>"`.
fn contained_value(value: &Value) -> Result<(), Uncontained> {
    let type_name: &str = match value {
        Value::Enum(OptionEnum::Some(value)) => &value.type_name,
        Value::Enum(OptionEnum::None(type_name)) => type_name,
        Value::Array(ArrayType::Enum(type_name), _) => type_name,
        _ => return Ok(()),
    };
    if type_name.contains(['"', '\0']) {
        Err(Uncontained::TypeName)
    } else {
        Ok(())
    }
}

/// The refusal of a condition or value of `E`'s, said at `warn` by entity and reason,
/// never by its text: the opaque failure every caller reads, sent before any
/// statement.
pub(crate) fn refused<E: EntityTrait>(why: Uncontained) -> DbErr {
    tracing::warn!(
        target: crate::target::ORM,
        entity = E::default().table_name(),
        reason = why.as_str(),
        "a condition or value handed to Repo refused — its text would not stay where Repo puts it",
    );
    DbErr::Custom(format!(
        "a condition or value handed to Repo was refused before any statement: its text \
         would not stay where Repo puts it ({}) — build it with sea-query's typed \
         calls, its values bound, every parenthesis, string and identifier it opens closed \
         inside it, and no comment",
        why.as_str()
    ))
}

/// What a byte outside every quoted region continues, which decides what a `$` or a
/// quote after it opens, as Postgres's lexer decides it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Word {
    None,
    Identifier { len: usize },
    Number,
}

impl Word {
    fn next(self, byte: u8) -> Self {
        let starts = byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80;
        match self {
            Self::Identifier { len } if starts || byte.is_ascii_digit() => {
                Self::Identifier { len: len + 1 }
            }
            Self::Number if starts || byte.is_ascii_digit() => Self::Number,
            _ if starts => Self::Identifier { len: 1 },
            _ if byte.is_ascii_digit() => Self::Number,
            _ => Self::None,
        }
    }
}

/// Whether `sql`, wrapped in one pair of parentheses, stays one operand as Postgres
/// reads it: every region it opens closes inside it, and it opens no region the scan
/// does not read the way Postgres does — dollar quoting, a comment, a backslash in a
/// standard string — which it refuses instead.
fn scan(sql: &[u8]) -> Result<(), Uncontained> {
    if sql.contains(&0) {
        return Err(Uncontained::Nul);
    }
    let mut depth = 0_usize;
    let mut word = Word::None;
    let mut at = 0;
    while let Some(&byte) = sql.get(at) {
        at += 1;
        match byte {
            b'(' => depth += 1,
            b')' => depth = depth.checked_sub(1).ok_or(Uncontained::Parenthesis)?,
            b'\'' => {
                // `E'…'` only where the `E` is a word of its own: `nameE'…'` is `nameE`.
                let escape =
                    word == Word::Identifier { len: 1 } && matches!(sql[at - 2], b'e' | b'E');
                at = string_end(sql, at, escape)?;
            }
            b'"' => at = identifier_end(sql, at)?,
            b'-' if sql.get(at) == Some(&b'-') => return Err(Uncontained::Comment),
            b'/' if sql.get(at) == Some(&b'*') => return Err(Uncontained::Comment),
            // `$` continues an identifier, as Postgres reads `a$b`.
            b'$' if let Word::Identifier { len } = word => {
                word = Word::Identifier { len: len + 1 };
                continue;
            }
            b'$' => {
                let digits = sql[at..].iter().take_while(|b| b.is_ascii_digit()).count();
                if digits == 0 {
                    return Err(Uncontained::Dollar);
                }
                at += digits;
                word = Word::Number;
                continue;
            }
            _ => {
                word = word.next(byte);
                continue;
            }
        }
        word = Word::None;
    }
    if depth == 0 {
        Ok(())
    } else {
        Err(Uncontained::Parenthesis)
    }
}

/// Where the string opened before `at` ends: a doubled quote is the string's, and in
/// an `escape` string so is the byte after a backslash.
fn string_end(sql: &[u8], mut at: usize, escape: bool) -> Result<usize, Uncontained> {
    while let Some(&byte) = sql.get(at) {
        at += 1;
        match byte {
            b'\\' if escape => at += 1,
            b'\\' => return Err(Uncontained::Backslash),
            b'\'' if sql.get(at) == Some(&b'\'') => at += 1,
            b'\'' => return Ok(at),
            _ => {}
        }
    }
    Err(Uncontained::String)
}

/// Where the identifier opened before `at` ends: a doubled quote is the identifier's.
fn identifier_end(sql: &[u8], mut at: usize) -> Result<usize, Uncontained> {
    while let Some(&byte) = sql.get(at) {
        at += 1;
        if byte == b'"' {
            if sql.get(at) == Some(&b'"') {
                at += 1;
            } else {
                return Ok(at);
            }
        }
    }
    Err(Uncontained::Identifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_condition_writes_and_closes_stays_one_operand() {
        for sql in [
            r#""widgets"."id" = $1"#,
            r#"lower("users"."email") = $1 AND ("a" OR "b")"#,
            r#"to_tsvector('english', "posts"."body") @@ plainto_tsquery('english', $1)"#,
            "a = 'it''s' OR b = E'it\\'s \\\\ fine'",
            r#""odd ""name""" IS NULL"#,
            "price$total > $12 AND a$ = 1",
            "x - -1 = 0 AND a / b = 2 AND a */ b",
            "nameE'x' IS NULL",
            "'a'\n'b' = 'ab'",
            "é = 1",
        ] {
            assert_eq!(scan(sql.as_bytes()), Ok(()), "{sql}");
        }
    }

    #[test]
    fn each_way_out_of_the_operand_is_refused_by_its_word() {
        for (sql, why) in [
            ("1 = 1) OR (1 = 1", Uncontained::Parenthesis),
            ("1 = 1)) OR ((1 = 1", Uncontained::Parenthesis),
            ("(1 = 1", Uncontained::Parenthesis),
            ("a = 'open", Uncontained::String),
            ("a = E'open\\'", Uncontained::String),
            ("a = E'ends in a backslash\\", Uncontained::String),
            (r#""open = 1"#, Uncontained::Identifier),
            (r#"'a'::"text" OR 1 = 1 --""#, Uncontained::Comment),
            ("1 = 1 /* ) OR ( */", Uncontained::Comment),
            ("a = 1 --", Uncontained::Comment),
            ("a = $$) OR ($$", Uncontained::Dollar),
            ("a = $q$) OR ($q$", Uncontained::Dollar),
            ("a = 1$q$x$q$", Uncontained::Dollar),
            ("a = $1$q$x$q$", Uncontained::Dollar),
            ("a = $", Uncontained::Dollar),
            ("a = 'x\\' ) OR (1 = 1 OR '", Uncontained::Backslash),
            ("E'a'\n'b\\'' = 1", Uncontained::Backslash),
            ("U&'d\\0061t' IS NULL", Uncontained::Backslash),
            ("a = 1\0) OR (1 = 1", Uncontained::Nul),
        ] {
            assert_eq!(scan(sql.as_bytes()), Err(why), "{sql}");
        }
    }

    #[test]
    fn a_quoted_region_hides_what_it_holds() {
        for sql in [
            "a = ') OR ('",
            r#""a) OR (b" = 1"#,
            "a = E'\\') OR (\\''",
            "a = '--' AND b = '/*'",
            r#""--" = 1 AND "$$" = 2"#,
        ] {
            assert_eq!(scan(sql.as_bytes()), Ok(()), "{sql}");
        }
    }

    #[test]
    #[expect(clippy::disallowed_methods, reason = "the values under test")]
    fn an_enum_type_name_holding_a_quote_is_refused_and_any_other_value_passes() {
        let named = |type_name: &str| {
            Value::from(sea_orm::sea_query::Enum {
                type_name: type_name.into(),
                value: "a".into(),
            })
        };
        assert_eq!(contained_value(&named("tea")), Ok(()));
        assert_eq!(
            contained_value(&named(r#"tea" OR 1 = 1 OR "x"#)),
            Err(Uncontained::TypeName),
        );
        assert_eq!(
            contained_value(&Value::Enum(OptionEnum::None(r#"a"b"#.into()))),
            Err(Uncontained::TypeName),
        );
        assert_eq!(
            contained_value(&Value::Array(
                ArrayType::Enum(Box::new(r#"a"b"#.into())),
                None
            )),
            Err(Uncontained::TypeName),
        );
        assert_eq!(contained_value(&Value::from(r#"a " b"#)), Ok(()));
    }
}
