use std::sync::Arc;

use nest_rs::http::{controller, crud};

use crate::app_authn::AppAuthnGuard;
use crate::app_authz::AppAuthzGuard;
use crate::notifications::{Entity as NotificationEntity, Notification, NotificationsService};

#[controller(path = "/notifications")]
#[use_guards(AppAuthnGuard, AppAuthzGuard)]
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
