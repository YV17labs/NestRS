//! An item a body declares is in scope of its whole block, the answer's own
//! names included, and the attempt's type is read where the signature is.

use super::*;

#[injectable]
struct QuotingService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
    attempts: AtomicU32,
}

impl QuotingService {
    fn fails_first(&self) -> bool {
        self.attempts.fetch_add(1, Ordering::Relaxed) == 0
    }

    fn attempted(&self) -> u32 {
        self.attempts.swap(0, Ordering::Relaxed)
    }

    /// Run once, the error's name imported as another's.
    #[resilient(courier)]
    async fn once(&self, weight: &str) -> Result<u32, ResilienceError<CarrierDown>> {
        use std::num::ParseIntError as CarrierDown;
        self.fails_first();
        let weight = weight
            .parse()
            .map_err(|_unparsed: CarrierDown| super::CarrierDown)?;
        Ok(weight)
    }

    /// Sent again, `Result` imported as an alias of one parameter.
    #[resilient(courier, idempotent)]
    async fn again(&self, parcel: &str) -> Result<usize, ResilienceError<CarrierDown>> {
        use std::io::Result;
        if self.fails_first() {
            return Err(CarrierDown);
        }
        let read: Result<usize> = Ok(parcel.len());
        read.map_err(|_unread| CarrierDown)
    }

    /// Under a fallback and a token, the answer's alias declared as another
    /// type.
    #[resilient(courier, fallback = held, cancel = token)]
    async fn falling_back(&self, parcel: &str) -> Answer<usize> {
        type Answer<T> = Option<T>;
        let found: Answer<usize> =
            (!token.is_cancelled() && !self.fails_first()).then_some(parcel.len());
        found.ok_or(CarrierDown)
    }

    async fn held(&self, parcel: &str, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        Ok(parcel.len() * 10)
    }

    #[resilient(courier, idempotent)]
    #[tracing::instrument(skip_all)]
    async fn below(&self, weight: &str) -> Result<u32, ResilienceError<CarrierDown>> {
        use std::num::ParseIntError as CarrierDown;
        if self.fails_first() {
            return Err(super::CarrierDown);
        }
        let weight = weight
            .parse()
            .map_err(|_unparsed: CarrierDown| super::CarrierDown)?;
        Ok(weight)
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, retrying_once()))],
    providers = [QuotingService],
)]
struct QuotingModule;

#[tokio::test(start_paused = true)]
async fn a_body_declaring_an_item_of_its_answers_name_runs_under_its_policy() {
    let app = boot::<QuotingModule>().await;
    let quoting = app
        .container()
        .get::<QuotingService>()
        .expect("the module provides it");

    let once = quoting.once("12").await;
    assert!(matches!(once, Ok(12)), "{once:?}");
    let refused = quoting.once("heavy").await;
    assert!(failed(&refused), "{refused:?}");
    assert_eq!(quoting.attempted(), 2, "each sent once");

    let again = quoting.again("P1").await;
    assert!(matches!(again, Ok(2)), "{again:?}");
    assert_eq!(quoting.attempted(), 2, "sent again after its failure");

    let fallen_back = quoting.falling_back("P1").await;
    assert!(matches!(fallen_back, Ok(20)), "{fallen_back:?}");
    assert_eq!(quoting.attempted(), 1, "its fallback answers for it");

    let below = quoting.below("12").await;
    assert!(matches!(below, Ok(12)), "{below:?}");
    assert_eq!(quoting.attempted(), 2, "sent again after its failure");
}
