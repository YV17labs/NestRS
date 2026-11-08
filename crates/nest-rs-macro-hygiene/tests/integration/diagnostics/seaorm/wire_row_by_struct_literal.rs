//! A wire row is built by its judgement alone: its proof is a private field no
//! struct literal supplies, so a body cannot hand a client a row no ability judged.

mod thing {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Thing")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "things")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

fn invented() -> thing::Thing {
    thing::Thing {
        id: String::new(),
        name: "invented".to_owned(),
    }
}

fn main() {
    let _ = invented();
}
