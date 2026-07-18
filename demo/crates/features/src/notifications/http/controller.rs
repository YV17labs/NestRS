use std::sync::Arc;

use nest_rs_http::{controller, crud};

use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::notifications::{Entity as NotificationEntity, Notification, NotificationsService};

/// Read-only resource: `GET /notifications` (list) and `GET /notifications/{id}`.
/// `ops = [list, get]` generates exactly those two read operations — no create,
/// update or delete route exists, matching a service that implements only
/// [`CrudService`](nest_rs_seaorm::CrudService). Both delegate through the
/// service, so reads are row-level scoped and masked by the caller's ability.
#[controller(path = "/notifications")]
#[use_guards(AuthGuard, AuthzGuard)]
pub struct NotificationsController {
    #[inject]
    svc: Arc<NotificationsService>,
}

#[crud(
    service = svc,
    entity = NotificationEntity,
    output = Notification,
    ops = [list, get],
)]
impl NotificationsController {}
