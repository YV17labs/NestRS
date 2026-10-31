use std::fmt;

use nest_rs::authn::PrincipalIdentity;
use nest_rs::seaorm::wire_enum;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[wire_enum]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Subject {
    User(Uuid),
    Client(String),
}

impl Subject {
    pub fn user_id(&self) -> Option<Uuid> {
        match self {
            Self::User(id) => Some(*id),
            Self::Client(_) => None,
        }
    }

    pub fn client_id(&self) -> Option<&str> {
        match self {
            Self::User(_) => None,
            Self::Client(id) => Some(id),
        }
    }
}

impl From<String> for Subject {
    fn from(sub: String) -> Self {
        match Uuid::try_parse(&sub) {
            Ok(id) => Self::User(id),
            Err(_) => Self::Client(sub),
        }
    }
}

impl From<Subject> for String {
    fn from(sub: Subject) -> Self {
        sub.to_string()
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(id) => id.fmt(f),
            Self::Client(id) => f.write_str(id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PrincipalIdentity)]
pub struct Claims {
    #[principal(actor)]
    pub sub: Subject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[principal(tenant)]
    pub org_id: Uuid,
    pub roles: Vec<Role>,
    #[principal(scopes)]
    #[serde(
        default,
        rename = "scope",
        with = "nest_rs::authn::scope::space_delimited",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub scopes: Vec<String>,
}

impl Claims {
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(roles: Vec<Role>) -> Claims {
        Claims {
            sub: Subject::User(Uuid::nil()),
            client_id: None,
            org_id: Uuid::nil(),
            roles,
            scopes: Vec::new(),
        }
    }

    fn machine(client_id: &str) -> Claims {
        Claims {
            sub: Subject::Client(client_id.to_owned()),
            client_id: Some(client_id.to_owned()),
            ..claims(vec![Role::User])
        }
    }

    #[test]
    fn actor_id_is_the_subject_of_either_kind() {
        assert_eq!(claims(vec![]).actor_id(), Some(Uuid::nil().to_string()));
        assert_eq!(machine("ci").actor_id().as_deref(), Some("ci"));
    }

    #[test]
    fn tenant_id_is_the_org_id_claim() {
        let org_id = Uuid::now_v7();
        let member = Claims {
            org_id,
            ..claims(vec![Role::User])
        };
        assert_eq!(member.tenant_id(), Some(org_id.to_string()));
        let machine = Claims {
            org_id,
            ..machine("ci")
        };
        assert_eq!(machine.tenant_id(), Some(org_id.to_string()));
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
    fn a_machine_grant_names_its_client_as_sub_and_client_id() {
        let json = serde_json::to_value(machine("ci")).expect("serialize");
        assert_eq!(json["sub"], "ci");
        assert_eq!(json["client_id"], "ci");

        let back: Claims = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.sub, Subject::Client("ci".into()));
        assert_eq!(back.sub.user_id(), None);
    }

    #[test]
    fn user_grant_carries_sub_through_round_trip() {
        use crate::authz::constants::POSTS_READ;
        let sub = Uuid::now_v7();
        let user = Claims {
            sub: Subject::User(sub),
            client_id: None,
            org_id: Uuid::now_v7(),
            roles: vec![Role::User],
            scopes: vec![POSTS_READ.into()],
        };
        let json = serde_json::to_value(&user).expect("serialize");
        assert_eq!(json["sub"], sub.to_string());
        assert!(json.get("client_id").is_none(), "{json}");
        assert_eq!(
            json["scope"], POSTS_READ,
            "the claim goes out in the space-delimited form RFC 6749 §3.3 defines",
        );
        let back: Claims = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.sub.user_id(), Some(sub));
        assert_eq!(back.scopes, [POSTS_READ]);
    }

    #[test]
    fn a_subject_a_uuid_parses_is_a_user() {
        let id = Uuid::now_v7();
        assert_eq!(Subject::from(id.to_string()), Subject::User(id));
        assert_eq!(Subject::from(id.simple().to_string()), Subject::User(id));
        assert_eq!(
            Subject::from("demo-service".to_owned()),
            Subject::Client("demo-service".into())
        );
    }

    #[test]
    fn a_token_naming_no_subject_is_refused_by_the_claims() {
        let mut json = serde_json::to_value(claims(vec![Role::Admin])).expect("serialize");
        json.as_object_mut().expect("an object").remove("sub");
        assert!(serde_json::from_value::<Claims>(json).is_err());
    }

    #[test]
    fn a_token_with_no_scope_claim_is_delegated_nothing() {
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
