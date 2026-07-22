//! [`Bind`] as a [`RouteResponseShaper`] — route-model binding authorizes the
//! row; this shaper applies the same field-level masking as
//! [`nest_rs_authz::http::Authorize`].

use std::future::Future;
use std::sync::Arc;

use nest_rs_authz::http::mask_entity_response;
use nest_rs_authz::{Ability, ActionMarker, with_ability};
use nest_rs_http::RouteResponseShaper;
use nest_rs_resource::WireModelDefaults;
use poem::{Request, Response, Result};
use sea_orm::EntityTrait;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::Bind;
use crate::CrudService;

impl<A, S> RouteResponseShaper for Bind<A, S>
where
    S: CrudService,
    A: ActionMarker,
    S::Entity: EntityTrait + WireModelDefaults,
    <S::Entity as EntityTrait>::Model: DeserializeOwned + Serialize,
{
    type Captured = Option<Arc<Ability>>;

    fn capture(req: &Request) -> Self::Captured {
        req.extensions().get::<Arc<Ability>>().cloned()
    }

    async fn run<F>(captured: Self::Captured, inner: F) -> Result<Response>
    where
        F: Future<Output = Result<Response>> + Send,
    {
        match captured {
            Some(ability) => {
                let resp = with_ability(ability.clone(), inner).await?;
                Ok(mask_entity_response::<S::Entity>(&ability, A::ACTION, resp).await)
            }
            None => inner.await,
        }
    }
}
