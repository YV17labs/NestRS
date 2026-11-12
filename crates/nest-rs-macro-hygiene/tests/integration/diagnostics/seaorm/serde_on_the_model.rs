//! A row deriving serde writes every column, hidden ones included, onto a wire,
//! and reads every column off a body no input validates: each
//! derive is refused where it is written — `#[derive(…)]`, and sea-orm's
//! `model_attrs(…)` and `model_ex_attrs(…)` — naming what answers instead.

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "posts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose(input(create, update))]
        pub title: String,
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod org {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Org")]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "orgs",
        model_attrs(derive(PartialEq, serde::Serialize)),
        model_ex_attrs(derive(serde::Serialize))
    )]
    pub struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose]
        pub name: String,
        pub plan: Option<String>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

fn main() {}
