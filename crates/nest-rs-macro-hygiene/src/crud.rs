//! `#[crud]` on a controller whose source names `std`, the umbrella and
//! `crate::`, and nothing else.
//!
//! The controller is where the decorator lives, so it is the manifest that has
//! to stay empty: unlike the entity, its source writes nothing an expansion
//! could lean on. Every operation is generated, so every route's emission is
//! compiled here.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::{controller, crud};
use nest_rs::seaorm::{Creatable, CrudService, Deletable, Updatable};

use crate::entity::{
    CreateHygieneNote, Entity as HygieneNoteEntity, HygieneNote, UpdateHygieneNote,
};

/// The audited API `#[crud]` calls into.
#[injectable]
#[derive(Default)]
pub struct HygieneCrudService;

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

/// The controller the five generated routes mount on.
#[controller(path = "/notes")]
pub struct HygieneCrudController {
    #[inject]
    svc: Arc<HygieneCrudService>,
}

#[crud(
    service = svc,
    entity = HygieneNoteEntity,
    output = HygieneNote,
    create = CreateHygieneNote,
    update = UpdateHygieneNote,
)]
impl HygieneCrudController {}
