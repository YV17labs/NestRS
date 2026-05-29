use std::sync::Arc;

use nestrs_http::{controller, crud};

use crate::authn::AuthGuard;
use crate::authz::AppAbilityGuard;
use crate::orgs::entity::{self, CreateOrgInput, Org, UpdateOrgInput};
use crate::orgs::service::OrgsService;

/// Orgs expose the full CRUD surface with no hand-written handler: `#[crud]`
/// generates `list`/`get`/`create`/`update`/`delete`, all **delegating to
/// `OrgsService`** (the entity's single ORM gateway) — the controller holds no
/// query. Security is declared once at the controller level
/// (`guards(AuthGuard, AppAbilityGuard)`), so every generated route inherits it
/// with nothing repeated per operation. Orgs are the tenant root with no
/// server-side scope column, so the service's inherited `create`/`update` map
/// cleanly from the `#[expose]` conversions.
///
/// Contrast `UsersController`, whose `create` is hand-written to stamp the
/// caller's `org_id` — but it shares the same controller-level guards.
#[controller(path = "/orgs")]
#[use_guards(AuthGuard, AppAbilityGuard)]
pub struct OrgsController {
    #[inject]
    svc: Arc<OrgsService>,
}

#[crud(
    service = svc,
    entity = entity::Entity,
    output = Org,
    create = CreateOrgInput,
    update = UpdateOrgInput,
    paginate = cursor,
)]
impl OrgsController {}
