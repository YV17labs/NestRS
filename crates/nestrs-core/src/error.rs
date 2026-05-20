use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

// `figment::Error` is ~208 bytes, so the `Err` variant of any `Result<_, Error>`
// would inflate every fallible signature. Boxing keeps `Result` small enough
// that `clippy::result_large_err` stays quiet without paying the cost on the
// hot (Ok) path.
#[derive(Debug, Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(Box<figment::Error>),

    #[error("{0}")]
    Internal(String),
}

impl From<figment::Error> for Error {
    fn from(value: figment::Error) -> Self {
        Self::Config(Box::new(value))
    }
}
