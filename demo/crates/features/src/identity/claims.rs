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
    #[serde(
        default,
        rename = "scope",
        with = "nest_rs::authn::scope::space_delimited",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub scopes: Vec<String>,
    pub exp: u64,
}

impl Claims {
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}

impl nest_rs::authn::PrincipalIdentity for Claims {
    fn actor_id(&self) -> Option<String> {
        self.sub.map(|sub| sub.to_string())
    }

    fn scopes(&self) -> Option<&[String]> {
        Some(&self.scopes)
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
            scopes: Vec::new(),
            exp: 0,
        }
    }

    #[test]
    fn actor_id_is_the_sub_claim() {
        use nest_rs::authn::PrincipalIdentity;
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
            scopes: Vec::new(),
            exp: 42,
        };
        let json = serde_json::to_value(&machine).expect("serialize");
        let obj = json.as_object().expect("object");
        assert!(!obj.contains_key("sub"), "machine grants omit sub: {obj:?}");
    }

    #[test]
    fn user_grant_carries_sub_through_round_trip() {
        use crate::authz::constants::POSTS_READ;
        let sub = Uuid::now_v7();
        let user = Claims {
            sub: Some(sub),
            org_id: Uuid::now_v7(),
            roles: vec![Role::User],
            scopes: vec![POSTS_READ.into()],
            exp: 100,
        };
        let json = serde_json::to_value(&user).expect("serialize");
        assert_eq!(
            json["scope"], POSTS_READ,
            "the claim goes out in the space-delimited form RFC 6749 §3.3 defines",
        );
        let back: Claims = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.sub, Some(sub));
        assert_eq!(back.exp, 100);
        assert_eq!(back.scopes, [POSTS_READ]);
    }

    #[test]
    fn a_token_with_no_scope_claim_is_delegated_nothing() {
        use nest_rs::authn::PrincipalIdentity;
        let bare = claims(vec![Role::Admin]);
        assert_eq!(
            bare.scopes(),
            Some([].as_slice()),
            "an admin token delegated nothing exercises nothing scope-gated",
        );
    }

    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Role::Admin).unwrap(), "\"admin\"");
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    }
}
