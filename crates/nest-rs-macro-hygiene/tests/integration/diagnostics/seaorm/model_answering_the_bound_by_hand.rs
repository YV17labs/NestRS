//! The bound `#[expose]`'s wire impls carry is no door a crate can open: both of its
//! names are foreign to the crate writing the answer, whatever it names for `Reply`.

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "posts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        pub author_email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use nest_rs::seaorm::__private::{AModelCrossesNoWire, HiddenColumns};

impl<'a> AModelCrossesNoWire<fn(post::Model) -> post::Post> for HiddenColumns<'a> {
    fn refused() -> ! {
        loop {}
    }
}

fn main() {}
