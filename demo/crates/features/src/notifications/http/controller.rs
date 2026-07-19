use std::sync::Arc;

use nest_rs_http::{controller, crud};

use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::notifications::{Entity as NotificationEntity, Notification, NotificationsService};

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
