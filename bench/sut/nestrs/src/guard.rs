use nest_rs::core::{Layer, UnitContext, async_trait, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor};
use nest_rs::http::{HttpView, header};

#[injectable]
#[derive(Layer)]
pub struct BearerGuard;

#[async_trait]
impl Guard for BearerGuard {
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let bearer = HttpView::of(cx)?
            .request()
            .headers()
            .get(header::AUTHORIZATION)
            .is_some_and(|value| value == "Bearer bench");
        match bearer {
            true => Ok(()),
            false => Err(Denial::unauthorized("a bearer token is required")),
        }
    }
}

impl GuardFor<HttpView> for BearerGuard {}
