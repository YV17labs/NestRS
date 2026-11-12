use nest_rs::core::injectable;
use nest_rs::seaorm::{Creatable, CrudService};

use super::entities::publication;

#[injectable]
pub struct PublicationStore;

impl CrudService for PublicationStore {
    type Entity = publication::Entity;
}

impl Creatable for PublicationStore {
    type Create = publication::ActiveModel;
}
