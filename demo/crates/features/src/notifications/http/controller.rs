use std::sync::Arc;

use nest_rs::http::{controller, crud};

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::notifications::NotificationsService;

#[controller(path = "/notifications")]
#[use_guards(AuthnGuard, AuthzGuard)]
pub struct NotificationsController {
    #[inject]
    svc: Arc<NotificationsService>,
}

#[crud(service = svc, ops = [list, get])]
impl NotificationsController {}
