use std::sync::Arc;

use features::orgs::{Column, Entity, OrgsService};
use features::testing::seed_org;
use nest_rs::authz::{Ability, AbilityBuilder, Action};
use nest_rs::seaorm::{Access, CrudService};

use crate::{connection, data_app};

fn read_all_ability() -> Arc<Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, Entity);
    Arc::new(b.build().expect("valid test ability"))
}

#[tokio::test]
async fn list_returns_seeded_orgs_for_an_unrestricted_reader() {
    let app = data_app().await;
    let org_id = seed_org(&connection(app.container()), "Acme").await;

    let rows = app
        .unit(read_all_ability(), || OrgsService.list())
        .await
        .expect("list succeeds");
    assert!(
        rows.iter()
            .any(|row| row.id == org_id && row.name == "Acme")
    );
}

#[tokio::test]
async fn access_hides_out_of_scope_orgs() {
    let app = data_app().await;
    let conn = connection(app.container());
    let allowed = seed_org(&conn, "Allowed").await;
    let blocked = seed_org(&conn, "Blocked").await;

    let ability = Arc::new({
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, Entity)
            .when(move |p| p.eq(Column::Id, allowed));
        b.build().expect("valid test ability")
    });

    let (found, denied) = app
        .unit(ability, move || async move {
            let service = OrgsService;
            Ok::<_, sea_orm::DbErr>((
                service.access(Action::Read, allowed).await?,
                service.access(Action::Read, blocked).await?,
            ))
        })
        .await
        .expect("both reads run");
    assert!(matches!(found, Access::Found(_)));
    assert!(matches!(denied, Access::Denied));
}
