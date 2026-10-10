use std::sync::Arc;

use nest_rs_authz::http::AbilityShaping;
use nest_rs_authz::{Ability, ActionMarker, WireModelDefaults};
use nest_rs_http::{ResponseShaping, RouteResponseShaper};
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
