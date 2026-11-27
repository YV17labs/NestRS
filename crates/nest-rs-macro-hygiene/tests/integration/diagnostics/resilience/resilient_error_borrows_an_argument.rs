//! The policy reads an error's chain as one of `dyn Error + 'static`, so a
//! decorated method's error owns what it says: one borrowing an argument is
//! rustc's lifetime error at the answer, run once or again, and one generic
//! over a type not `'static` its `E0310`, whose help adds the bound. The
//! error owning its value compiles.

use std::error::Error;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult};
use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

#[derive(Debug)]
struct Unparsed<'a>(&'a str);

impl Display for Unparsed<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "`{}` is no weight", self.0)
    }
}

impl Error for Unparsed<'_> {}

#[derive(Debug)]
struct Refused<T>(T);

impl<T: Debug> Display for Refused<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{:?} was refused", self.0)
    }
}

impl<T: Debug> Error for Refused<T> {}

#[injectable]
struct ShipmentsService {
    #[inject]
    carrier: Arc<ResiliencePolicy>,
}

impl ShipmentsService {
    #[resilient(carrier)]
    async fn weigh<'a>(&self, weight: &'a str) -> Result<u32, ResilienceError<Unparsed<'a>>> {
        weight.parse().map_err(|_| Unparsed(weight))
    }

    #[resilient(carrier, idempotent)]
    async fn weigh_again<'a>(&self, weight: &'a str) -> Result<u32, ResilienceError<Unparsed<'a>>> {
        weight.parse().map_err(|_| Unparsed(weight))
    }

    #[resilient(carrier)]
    async fn label<T: Debug + Send + Sync>(
        &self,
        parcel: T,
    ) -> Result<(), ResilienceError<Refused<T>>> {
        Err(Refused(parcel))
    }

    #[resilient(carrier)]
    async fn weigh_owning(&self, weight: &str) -> Result<u32, ResilienceError<Refused<String>>> {
        weight.parse().map_err(|_| Refused(weight.to_owned()))
    }

    #[resilient(carrier)]
    async fn label_owning<T: Debug + Send + Sync + 'static>(
        &self,
        parcel: T,
    ) -> Result<(), ResilienceError<Refused<T>>> {
        Err(Refused(parcel))
    }
}

fn main() {}
