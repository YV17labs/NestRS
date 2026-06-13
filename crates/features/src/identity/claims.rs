use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<Uuid>,
    pub org_id: Uuid,
    pub roles: Vec<Role>,
    pub exp: u64,
}

impl Claims {
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}

/// Audit identity: the `sub` claim. A subject-less token (e.g. a
/// client-credentials grant carried in these claims) has no actor id.
impl nest_rs_authn::PrincipalIdentity for Claims {
    fn actor_id(&self) -> Option<String> {
        self.sub.map(|sub| sub.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(roles: Vec<Role>) -> Claims {
        Claims {
            sub: Some(Uuid::nil()),
            org_id: Uuid::nil(),
            roles,
            exp: 0,
        }
    }

    #[test]
    fn actor_id_is_the_sub_claim() {
        use nest_rs_authn::PrincipalIdentity;
        let with_sub = claims(vec![]);
        assert_eq!(with_sub.actor_id(), Some(Uuid::nil().to_string()));
        let mut subjectless = claims(vec![]);
        subjectless.sub = None;
        assert_eq!(subjectless.actor_id(), None);
    }

    #[test]
    fn admin_role_grants_admin() {
        assert!(claims(vec![Role::Admin]).is_admin());
    }

    #[test]
    fn user_role_alone_does_not_grant_admin() {
        assert!(!claims(vec![Role::User]).is_admin());
    }

    #[test]
    fn mixed_roles_with_admin_grant_admin() {
        assert!(claims(vec![Role::User, Role::Admin]).is_admin());
    }

    #[test]
    fn empty_roles_do_not_grant_admin() {
        assert!(!claims(vec![]).is_admin());
    }

    #[test]
    fn machine_grant_omits_sub_from_the_wire() {
        let machine = Claims {
            sub: None,
            org_id: Uuid::nil(),
            roles: vec![Role::User],
            exp: 42,
        };
        let json = serde_json::to_value(&machine).expect("serialize");
        let obj = json.as_object().expect("object");
        assert!(!obj.contains_key("sub"), "machine grants omit sub: {obj:?}");
    }

    #[test]
    fn user_grant_carries_sub_through_round_trip() {
        let sub = Uuid::now_v7();
        let user = Claims {
            sub: Some(sub),
            org_id: Uuid::now_v7(),
            roles: vec![Role::User],
            exp: 100,
        };
        let json = serde_json::to_value(&user).expect("serialize");
        let back: Claims = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.sub, Some(sub));
        assert_eq!(back.exp, 100);
    }

    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), "\"admin\"");
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    }
}
