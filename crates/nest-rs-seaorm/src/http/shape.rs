//! [`Bind`] as a [`RouteResponseShaper`] — route-model binding authorizes the
//! row; this shaper applies the same field-level masking as
//! [`nest_rs_authz::http::Authorize`].

use std::sync::Arc;

use nest_rs_authz::http::AbilityShaping;
use nest_rs_authz::{Ability, ActionMarker};
use nest_rs_http::{ResponseShaping, RouteResponseShaper};
use nest_rs_resource::WireModelDefaults;
use poem::Request;
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
    fn capture(req: &Request) -> Option<Box<dyn ResponseShaping>> {
        let ability = req.extensions().get::<Arc<Ability>>().cloned()?;
        Some(Box::new(AbilityShaping::<S::Entity>::new(
            ability,
            A::ACTION,
        )))
    }
}
