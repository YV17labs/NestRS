//! `#[crud]` on a controller whose source names `std`, the umbrella and
//! `crate::`, and nothing else; every generated route's emission compiles here.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::{controller, crud};
use nest_rs::seaorm::{Creatable, CrudService, Deletable, Updatable};

use crate::entity::{
    CreateHygieneNote, Entity as HygieneNoteEntity, HygieneNote, UpdateHygieneNote,
};

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
