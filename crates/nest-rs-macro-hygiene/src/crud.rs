//! `#[crud]` on a controller, and under `graphql` on a resolver, whose source
//! names `std`, the umbrella and `crate::`, and nothing else; every generated
//! operation's emission compiles here, its types read off the service — a
//! crate-private one, which each public host injects — and its field reached
//! by its type, whatever path the block names the host by.

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

#[crud(service = HygieneCrudService)]
impl HygieneCrudController {}

/// A host whose block is written in a module below the struct's, by path.
#[controller(path = "/notes/read-only")]
pub struct HygieneReadOnlyCrudController {
    #[inject]
    svc: Arc<HygieneCrudService>,
}

mod read_only {
    use nest_rs::http::crud;

    #[crud(service = super::HygieneCrudService, ops = [list, get])]
    impl super::HygieneReadOnlyCrudController {}
}

/// A host whose block names it through a type alias.
#[controller(path = "/notes/aliased")]
pub struct HygieneAliasedCrudController {
    #[inject]
    svc: Arc<HygieneCrudService>,
}

type HygieneAliased = HygieneAliasedCrudController;

#[crud(service = HygieneCrudService, ops = [get])]
impl HygieneAliased {}

/// A host whose block is written in a module below the struct's, through a
/// named import.
#[controller(path = "/notes/imported")]
pub struct HygieneImportedCrudController {
    #[inject]
    svc: Arc<HygieneCrudService>,
}

mod imported {
    use nest_rs::http::crud;

    use super::{HygieneCrudService, HygieneImportedCrudController};

    #[crud(service = HygieneCrudService, ops = [get])]
    impl HygieneImportedCrudController {}
}

/// A host whose block is written in a module that is not below the struct's.
pub mod host {
    use std::sync::Arc;

    use nest_rs::http::controller;

    use super::HygieneCrudService;

    #[controller(path = "/notes/elsewhere")]
    pub struct HygieneElsewhereCrudController {
        #[inject]
        svc: Arc<HygieneCrudService>,
    }
}

mod elsewhere {
    use nest_rs::http::crud;

    use super::HygieneCrudService;

    #[crud(service = HygieneCrudService, ops = [get])]
    impl super::host::HygieneElsewhereCrudController {}
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

    #[crud(service = HygieneCrudService)]
    impl HygieneNotesResolver {}

    /// A resolver whose block names it through a type alias.
    #[resolver]
    pub struct HygieneAliasedNotesResolver {
        #[inject]
        svc: Arc<HygieneCrudService>,
    }

    type HygieneAliasedNotes = HygieneAliasedNotesResolver;

    #[crud(service = HygieneCrudService, ops = [get])]
    impl HygieneAliasedNotes {}
}
