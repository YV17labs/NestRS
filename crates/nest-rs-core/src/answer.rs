//! What a handler answered, known by its type — never by how the type is spelled.
//!
//! A decorator reads a handler's return type as tokens, and tokens cannot see
//! through a name: `use async_graphql::Result as GqlResult`, `use poem::Result
//! as PoemResult` and a type alias all spell a `Result` a macro takes for an
//! ordinary value. Read that way, an error became a value — reaching the client
//! as one while the operation line filed `ok` (GraphQL), or having its status
//! rewritten by `#[http_code]` into a success (HTTP). Method resolution sees
//! the type the alias names, so the expansions ask this probe instead of the
//! spelling.
//!
//! The shape is `nest_rs_pipes::ValidateProbe`'s: inherent methods, which method
//! resolution prefers, exist only for a `Result`, and [`AnswerFallback`] answers
//! for every other type. The probe is zero-sized and resolved by the compiler —
//! the request pays nothing for it. A return type naming a generic parameter
//! resolves to the fallback, since nothing proves the parameter a `Result`.
//!
//! The edges that ask it are GraphQL's `#[operations]`, HTTP's `#[routes]` and
//! MCP's `#[tools]`. WebSocket's `#[messages]` answers the same question with
//! its own probe, `nest_rs_ws::ReplyValue`, because a WS reply splits twice and
//! reports through tiers no other edge has.

/// A handler's answer, borrowed so the compiler can be asked what it is.
#[doc(hidden)]
pub struct Answer<'a, R>(pub &'a R);

/// What [`Answer::kind`] says of a `Result`, whatever it is called.
#[doc(hidden)]
pub struct ResultAnswer;

/// What [`AnswerFallback::kind`] says of anything else.
#[doc(hidden)]
pub struct ValueAnswer;

/// A `Result` answer kept whole inside the edge's own `Result` — what a split
/// and a mask hand on, so the edge can still tell the two failures apart.
type Lifted<T, E, X> = Result<Result<T, E>, X>;

/// What [`Answer::mapper`] hands back: `f` applied to the value inside a
/// `Result` answer, the answer kept whole.
type Masking<T, E, X, F> = fn(Result<T, E>, F) -> Lifted<T, E, X>;

impl<T, E> Answer<'_, Result<T, E>> {
    /// A `Result` answer with its error lifted out into the edge's own, so a
    /// failure stays one whatever the type is called. The value keeps its type.
    pub fn split<X: From<E>>(&self) -> fn(Result<T, E>) -> Lifted<T, E, X> {
        |answer| match answer {
            Ok(value) => Ok(Ok(value)),
            Err(error) => Err(X::from(error)),
        }
    }

    /// Turn the value inside a `Result` answer into what the edge sends, the
    /// error lifted out into the edge's own — a response built from the value
    /// alone, so nothing a decorator applies to a success reaches a failure.
    pub fn map<X: From<E>, U, F>(&self) -> fn(Result<T, E>, F) -> Result<U, X>
    where
        F: FnOnce(T) -> U,
    {
        |answer, f| match answer {
            Ok(value) => Ok(f(value)),
            Err(error) => Err(X::from(error)),
        }
    }

    /// Apply `f` to the value inside a `Result` answer — what a mask reads, so
    /// it masks the row rather than the `Result` around it.
    pub fn mapper<X, F>(&self) -> Masking<T, E, X, F>
    where
        F: FnOnce(T) -> Result<T, X>,
    {
        |answer, f| match answer {
            Ok(value) => f(value).map(Ok),
            Err(error) => Ok(Err(error)),
        }
    }

    /// A `Result`.
    pub fn kind(&self) -> ResultAnswer {
        ResultAnswer
    }
}

/// The answers for every type the inherent [`Answer`] methods do not take.
#[doc(hidden)]
pub trait AnswerFallback {
    /// The answer's own type.
    type Value;

    /// A value answer, wrapped as it is.
    fn split<X>(&self) -> fn(Self::Value) -> Result<Self::Value, X>;

    /// Turn the answer itself into what the edge sends.
    fn map<X, U, F>(&self) -> fn(Self::Value, F) -> Result<U, X>
    where
        F: FnOnce(Self::Value) -> U;

    /// Apply `f` to the answer itself.
    fn mapper<X, F>(&self) -> fn(Self::Value, F) -> Result<Self::Value, X>
    where
        F: FnOnce(Self::Value) -> Result<Self::Value, X>;

    /// A value.
    fn kind(&self) -> ValueAnswer;
}

impl<R> AnswerFallback for Answer<'_, R> {
    type Value = R;

    fn split<X>(&self) -> fn(R) -> Result<R, X> {
        Ok
    }

    fn map<X, U, F>(&self) -> fn(R, F) -> Result<U, X>
    where
        F: FnOnce(R) -> U,
    {
        |answer, f| Ok(f(answer))
    }

    fn mapper<X, F>(&self) -> fn(R, F) -> Result<R, X>
    where
        F: FnOnce(R) -> Result<R, X>,
    {
        |answer, f| f(answer)
    }

    fn kind(&self) -> ValueAnswer {
        ValueAnswer
    }
}

#[cfg(test)]
mod tests {
    use super::{Answer, AnswerFallback as _, ResultAnswer, ValueAnswer};

    type Renamed<T> = Result<T, String>;

    /// A type whose name ends in `Result`.
    #[derive(Debug, PartialEq)]
    struct SearchResult(i32);

    /// The probe answers by type: a `Result` under another name is a `Result`,
    /// and a type whose name ends in `Result` is a value.
    #[test]
    fn a_result_is_known_by_its_type_never_by_its_name() {
        let renamed: Renamed<i32> = Ok(1);
        let _: ResultAnswer = Answer(&renamed).kind();
        let _: ValueAnswer = Answer(&SearchResult(1)).kind();
        let _: ValueAnswer = Answer(&Some(1)).kind();
        let _: ValueAnswer = Answer(&vec![1]).kind();
    }

    /// A `Result` answer's error is lifted out; a value is wrapped as it is.
    #[test]
    fn a_result_answer_splits_its_error_out() {
        let failed: Renamed<i32> = Err("refused".into());
        let split: Result<Renamed<i32>, String> = Answer(&failed).split()(failed);
        assert_eq!(split, Err("refused".to_owned()));

        let answered: Renamed<i32> = Ok(1);
        let split: Result<Renamed<i32>, String> = Answer(&answered).split()(answered);
        assert_eq!(split, Ok(Ok(1)));

        let value = SearchResult(2);
        let split: Result<SearchResult, String> = Answer(&value).split()(value);
        assert_eq!(split, Ok(SearchResult(2)));
    }

    /// `map` builds from the value alone, and a `Result` answer's error never
    /// reaches `f`.
    #[test]
    fn map_builds_from_the_value_and_lifts_the_error() {
        let failed: Renamed<i32> = Err("refused".into());
        let mapped: Result<String, String> =
            Answer(&failed).map()(failed, |value: i32| format!("sent {value}"));
        assert_eq!(mapped, Err("refused".to_owned()));

        let answered: Renamed<i32> = Ok(1);
        let mapped: Result<String, String> =
            Answer(&answered).map()(answered, |value: i32| format!("sent {value}"));
        assert_eq!(mapped, Ok("sent 1".to_owned()));

        let value = SearchResult(2);
        let mapped: Result<i32, String> =
            Answer(&value).map()(value, |value: SearchResult| value.0);
        assert_eq!(mapped, Ok(2));
    }

    /// The mapper reaches the value inside a `Result` answer, and the answer
    /// itself otherwise — what a mask is handed.
    #[test]
    fn the_mapper_reaches_the_value_inside_a_result() {
        let renamed: Renamed<i32> = Ok(1);
        let mapped: Result<Renamed<i32>, String> =
            Answer(&renamed).mapper()(renamed, |inner: i32| Ok(inner + 1));
        assert_eq!(mapped, Ok(Ok(2)));

        let value = SearchResult(2);
        let mapped: Result<SearchResult, String> =
            Answer(&value).mapper()(value, |inner: SearchResult| Ok(SearchResult(inner.0 + 1)));
        assert_eq!(mapped, Ok(SearchResult(3)));
    }
}
