use nest_rs::core::{Layer, UnitContext, async_trait, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor};
use nest_rs::http::{HttpView, header};

#[injectable]
#[derive(Default)]
pub struct BearerGuard;

impl Layer for BearerGuard {}

#[async_trait]
impl Guard for BearerGuard {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
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
