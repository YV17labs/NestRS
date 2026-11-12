//! `#[derive(PrincipalIdentity)]`: a claims type answers what the fields it
//! marks hold, as the two impls it replaces did, and travels as they let it.

use std::fmt;
use std::sync::Arc;

use nest_rs_authn::PrincipalIdentity;
use nest_rs_core::{AmbientValue, Continuation, ambient, with_ambient};
use uuid::Uuid;

/// A subject of either kind, one string on the wire.
#[derive(Clone, Debug, PartialEq)]
enum Subject {
    User(Uuid),
    Client(String),
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(id) => id.fmt(f),
            Self::Client(id) => f.write_str(id),
        }
    }
}

/// The claims a product derives its principal for.
#[derive(Clone, Debug, PrincipalIdentity)]
struct Claims {
    #[principal(actor)]
    sub: Subject,
    #[principal(tenant)]
    org_id: Uuid,
    #[principal(scopes)]
    scopes: Vec<String>,
    roles: Vec<String>,
}

/// The same claims with the impls written by hand, as the derive's users wrote
/// them before it.
#[derive(Clone, Debug)]
struct HandWritten {
    sub: Subject,
    org_id: Uuid,
    scopes: Vec<String>,
}

impl AmbientValue for HandWritten {}

impl PrincipalIdentity for HandWritten {
    fn actor_id(&self) -> Option<String> {
        Some(self.sub.to_string())
    }

    fn tenant_id(&self) -> Option<String> {
        Some(self.org_id.to_string())
    }

    fn scopes(&self) -> Option<&[String]> {
        Some(&self.scopes)
    }
}

fn both(sub: Subject, scopes: &[&str]) -> (Claims, HandWritten) {
    let org_id = Uuid::now_v7();
    let scopes: Vec<String> = scopes.iter().map(|&scope| scope.to_owned()).collect();
    let derived = Claims {
        sub: sub.clone(),
        org_id,
        scopes: scopes.clone(),
        roles: vec!["admin".to_owned()],
    };
    (
        derived,
        HandWritten {
            sub,
            org_id,
            scopes,
        },
    )
}

#[test]
fn a_derived_claims_type_answers_as_the_hand_written_impls_did() {
    for sub in [
        Subject::User(Uuid::now_v7()),
        Subject::Client("ci".to_owned()),
    ] {
        for scopes in [&[][..], &["posts:read", "audio:transcode"][..]] {
            let (derived, by_hand) = both(sub.clone(), scopes);
            assert_eq!(derived.actor_id(), by_hand.actor_id());
            assert_eq!(derived.tenant_id(), by_hand.tenant_id());
            assert_eq!(derived.scopes(), by_hand.scopes());
        }
    }
    let (derived, _) = both(Subject::Client("ci".to_owned()), &[]);
    assert_eq!(derived.actor_id().as_deref(), Some("ci"));
    assert_eq!(
        derived.scopes(),
        Some([].as_slice()),
        "a scope-aware credential granted nothing is delegated nothing",
    );
    assert_eq!(derived.roles, ["admin"]);
}

/// Only the subject is marked.
#[derive(Clone, PrincipalIdentity)]
struct Member {
    #[principal(actor)]
    id: String,
}

#[test]
fn a_role_left_unmarked_answers_the_traits_default() {
    let member = Member {
        id: "ada".to_owned(),
    };
    assert_eq!(member.actor_id().as_deref(), Some("ada"));
    assert_eq!(member.tenant_id(), None, "bound to no tenant");
    assert_eq!(member.scopes(), None, "not scope-aware");
}

/// A subject and a tenant a credential may leave out.
#[derive(Clone, PrincipalIdentity)]
struct Optional {
    #[principal(actor)]
    sub: Option<String>,
    #[principal(tenant)]
    org_id: Option<Uuid>,
}

#[test]
fn an_optional_field_answers_its_value_or_none() {
    let org_id = Uuid::now_v7();
    let named = Optional {
        sub: Some("ada".to_owned()),
        org_id: Some(org_id),
    };
    assert_eq!(named.actor_id().as_deref(), Some("ada"));
    assert_eq!(named.tenant_id(), Some(org_id.to_string()));

    let anonymous = Optional {
        sub: None,
        org_id: None,
    };
    assert_eq!(anonymous.actor_id(), None);
    assert_eq!(anonymous.tenant_id(), None);
}

/// A key, its owner and its tenant, and what it may do, read by position off
/// borrowed fields.
#[derive(Clone, PrincipalIdentity)]
struct ApiKey(
    #[principal(actor, tenant)] &'static str,
    #[principal(scopes)] &'static [String],
    u32,
);

#[test]
fn a_tuple_field_is_read_by_its_position_and_may_play_two_roles() {
    let key = ApiKey("acme", &[], 7);
    assert_eq!(key.actor_id().as_deref(), Some("acme"));
    assert_eq!(key.tenant_id().as_deref(), Some("acme"));
    assert_eq!(key.scopes(), Some([].as_slice()));
    assert_eq!(key.2, 7);
}

#[tokio::test]
async fn the_derived_claims_are_ambient_and_stay_out_of_a_jobs_envelope() {
    let (claims, _) = both(Subject::User(Uuid::nil()), &["posts:read"]);
    let carried = Arc::new(claims);
    with_ambient(Arc::clone(&carried), async {
        let read = ambient::<Claims>().expect("the claims are ambient");
        assert_eq!(read.actor_id(), Some(Uuid::nil().to_string()));
    })
    .await;
    assert!(Arc::clone(&carried).carry(Continuation::Task).is_some());
    assert!(
        carried.carry(Continuation::Envelope).is_none(),
        "a job's envelope carries the tenant and the actor, never the claims",
    );
}
