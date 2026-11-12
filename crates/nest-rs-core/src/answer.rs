//! What a handler answered, known by its type — never by how the type is spelled.
//!
//! A macro reading tokens cannot see through an alias (`http::Result as
//! Answered`), so the expansions ask this probe: inherent methods, which
//! method resolution prefers, exist only for a `Result`, and [`AnswerFallback`]
//! answers for every other type. A return type naming a generic parameter
//! resolves to the fallback.

/// A handler's answer, borrowed so the compiler can be asked what it is.
pub struct Answer<'a, R>(pub &'a R);

/// What [`Answer::kind`] says of a `Result`, whatever it is called.
pub struct ResultAnswer;

/// What [`AnswerFallback::kind`] says of anything else.
pub struct ValueAnswer;

/// A `Result` answer kept whole inside the edge's own `Result` — what a split
/// hands on, so the edge can still tell the two failures apart.
type Lifted<T, E, X> = Result<Result<T, E>, X>;

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
    /// error kept as it is, for the edge to read by its type — a response
    /// built from the value alone, so nothing a decorator applies to a success
    /// reaches a failure.
    pub fn map<U, F>(&self) -> fn(Result<T, E>, F) -> Result<U, E>
    where
        F: FnOnce(T) -> U,
    {
        |answer, f| answer.map(f)
    }

    /// A `Result`.
    pub fn kind(&self) -> ResultAnswer {
        ResultAnswer
    }
}

/// The answers for every type the inherent [`Answer`] methods do not take.
pub trait AnswerFallback {
    /// The answer's own type.
    type Value;

    /// A value answer, wrapped as it is.
    fn split<X>(&self) -> fn(Self::Value) -> Result<Self::Value, X>;

    /// Turn the answer itself into what the edge sends.
    fn map<U, F>(&self) -> fn(Self::Value, F) -> U
    where
        F: FnOnce(Self::Value) -> U;

    /// A value.
    fn kind(&self) -> ValueAnswer;
}

impl<R> AnswerFallback for Answer<'_, R> {
    type Value = R;

    fn split<X>(&self) -> fn(R) -> Result<R, X> {
        Ok
    }

    fn map<U, F>(&self) -> fn(R, F) -> U
    where
        F: FnOnce(R) -> U,
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

    #[derive(Debug, PartialEq)]
    struct SearchResult(i32);

    /// An error that converts into nothing, so only the answer's own can hold it.
    #[derive(Debug, PartialEq)]
    struct Refused;

    #[test]
    fn a_result_is_known_by_its_type_never_by_its_name() {
        let renamed: Renamed<i32> = Ok(1);
        let _: ResultAnswer = Answer(&renamed).kind();
        let _: ValueAnswer = Answer(&SearchResult(1)).kind();
        let _: ValueAnswer = Answer(&Some(1)).kind();
        let _: ValueAnswer = Answer(&vec![1]).kind();
    }

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

    #[test]
    fn map_builds_from_the_value_and_keeps_the_error() {
        let failed: Result<i32, Refused> = Err(Refused);
        let mapped: Result<String, Refused> =
            Answer(&failed).map()(failed, |value: i32| format!("sent {value}"));
        assert_eq!(mapped, Err(Refused));

        let answered: Renamed<i32> = Ok(1);
        let mapped: Renamed<String> =
            Answer(&answered).map()(answered, |value: i32| format!("sent {value}"));
        assert_eq!(mapped, Ok("sent 1".to_owned()));

        let value = SearchResult(2);
        let mapped: i32 = Answer(&value).map()(value, |value: SearchResult| value.0);
        assert_eq!(mapped, 2);
    }
}
