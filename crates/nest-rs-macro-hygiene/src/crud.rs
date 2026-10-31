//! `#[crud]` on a controller, and under `graphql` on a resolver, whose source
//! names `std`, the umbrella and `crate::`, and nothing else; every generated
//! operation's emission compiles here, its types read off the service — a
//! crate-private one, which each public host injects.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::{controller, crud};
use nest_rs::seaorm::{Creatable, CrudService, Deletable, Updatable};

use crate::entity::{CreateHygieneNote, Entity as HygieneNoteEntity, UpdateHygieneNote};

#[injectable]
#[derive(Default)]
pub(crate) struct HygieneCrudService;

impl CrudService for HygieneCrudService {
    type Entity = HygieneNoteEntity;
}

impl Creatable for HygieneCrudService {
    type Create = CreateHygieneNote;
}

impl Updatable for HygieneCrudService {
    type Update = UpdateHygieneNote;
}

impl Deletable for HygieneCrudService {}

#[controller(path = "/notes")]
pub struct HygieneCrudController {
    #[inject]
    svc: Arc<HygieneCrudService>,
}

#[crud(service = svc)]
impl HygieneCrudController {}

/// A host whose block is written in a module below the struct's, which reaches
/// its injected fields as the struct's own module does.
#[controller(path = "/notes/read-only")]
pub struct HygieneReadOnlyCrudController {
    #[inject]
    svc: Arc<HygieneCrudService>,
}

mod read_only {
    use nest_rs::http::crud;

    #[crud(service = svc, ops = [list, get])]
    impl super::HygieneReadOnlyCrudController {}
}

/// The GraphQL member of the pair, generating every operation.
#[cfg(feature = "graphql")]
pub mod graphql {
    use std::sync::Arc;

    use nest_rs::graphql::{crud, resolver};

    use super::HygieneCrudService;

    #[resolver]
    pub struct HygieneNotesResolver {
        #[inject]
        svc: Arc<HygieneCrudService>,
    }

    #[crud(service = svc)]
    impl HygieneNotesResolver {}
}
