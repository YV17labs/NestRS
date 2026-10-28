use sea_orm::sea_query::SelectStatement;
use sea_orm::{
    DbErr, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, QueryTrait, Select,
    SelectExt,
};

use crate::repo::Repo;

/// A query [`Repo`] built — [`scoped`](Repo::scoped), an escape such as
/// [`unscoped`](Repo::unscoped) — that runs on the ambient executor.
///
/// It is chained with sea-orm's own [`QueryFilter`], [`QueryOrder`] and
/// [`QuerySelect`], then run by [`all`](Self::all), [`one`](Self::one),
/// [`count`](Self::count) or [`exists`](Self::exists), each on the unit's
/// executor and failing closed outside one. It takes no connection, so the
/// statement it runs is the one `Repo` filtered — unless it was rewritten
/// through `query()`, which each of those traits hands out as `&mut`: a
/// statement replaced, cleared or `union`ed with another `SELECT` there runs
/// past the filter. Chain the builder methods; a `.query()` on a `Repo` select
/// is reviewed as an escape.
///
/// ```no_run
/// # mod post {
/// #     use sea_orm::entity::prelude::*;
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #     #[sea_orm(table_name = "posts")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         pub id: i32,
/// #         pub title: String,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// use nest_rs_authz::Action;
/// use nest_rs_seaorm::Repo;
/// use sea_orm::{ColumnTrait, QueryFilter, QueryOrder};
///
/// # async fn titled(title: &str) -> Result<(), sea_orm::DbErr> {
/// let posts = Repo::<post::Entity>::scoped(Action::Read)
///     .filter(post::Column::Title.eq(title))
///     .order_by_asc(post::Column::Id)
///     .all()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct RepoSelect<E: EntityTrait> {
    select: Select<E>,
}

impl<E: EntityTrait> RepoSelect<E> {
    /// `select`, as `Repo` filtered it.
    pub(crate) fn new(select: Select<E>) -> Self {
        Self { select }
    }

    /// Every row the query matches.
    pub async fn all(self) -> Result<Vec<E::Model>, DbErr> {
        let conn = Repo::<E>::conn()?;
        self.select.all(&conn).await
    }

    /// The first row the query matches, or `None`.
    pub async fn one(self) -> Result<Option<E::Model>, DbErr> {
        let conn = Repo::<E>::conn()?;
        self.select.one(&conn).await
    }

    /// How many rows the query matches.
    pub async fn count(self) -> Result<u64, DbErr>
    where
        E::Model: Sync,
    {
        let conn = Repo::<E>::conn()?;
        self.select.count(&conn).await
    }

    /// Whether the query matches any row.
    pub async fn exists(self) -> Result<bool, DbErr> {
        let conn = Repo::<E>::conn()?;
        self.select.exists(&conn).await
    }
}

impl<E: EntityTrait> QueryFilter for RepoSelect<E> {
    type QueryStatement = SelectStatement;

    fn query(&mut self) -> &mut SelectStatement {
        QueryTrait::query(&mut self.select)
    }
}

impl<E: EntityTrait> QueryOrder for RepoSelect<E> {
    type QueryStatement = SelectStatement;

    fn query(&mut self) -> &mut SelectStatement {
        QueryTrait::query(&mut self.select)
    }
}

impl<E: EntityTrait> QuerySelect for RepoSelect<E> {
    type QueryStatement = SelectStatement;

    fn query(&mut self) -> &mut SelectStatement {
        QueryTrait::query(&mut self.select)
    }
}

impl<E: EntityTrait> QueryTrait for RepoSelect<E> {
    type QueryStatement = SelectStatement;

    fn query(&mut self) -> &mut SelectStatement {
        QueryTrait::query(&mut self.select)
    }

    fn as_query(&self) -> &SelectStatement {
        self.select.as_query()
    }

    fn into_query(self) -> SelectStatement {
        self.select.into_query()
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_authz::Action;
    use sea_orm::DbErr;

    use crate::repo::Repo;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    type Widgets = Repo<widget::Entity>;

    /// What each way of running a query answered.
    async fn every_run() -> [Result<(), DbErr>; 4] {
        [
            Widgets::scoped(Action::Read).all().await.map(drop),
            Widgets::unscoped().one().await.map(drop),
            Widgets::with_tombstones(Action::Read)
                .count()
                .await
                .map(drop),
            Widgets::unscoped_by_id(7).exists().await.map(drop),
        ]
    }

    #[tokio::test]
    async fn a_query_outside_a_unit_fails_closed_for_want_of_an_executor() {
        for answer in every_run().await {
            assert!(
                matches!(&answer, Err(err) if err.to_string().contains("no ambient database executor")),
                "{answer:?}",
            );
        }
    }
}
