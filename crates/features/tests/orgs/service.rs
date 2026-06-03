use std::sync::Arc;

use features::orgs::{ActiveModel, Column, Entity, OrgsService};
use nestrs_authz::{with_ability, Ability, AbilityBuilder, Action};
use nestrs_database::{with_request_executor, Access, CrudService, Executor};
use nestrs_testing::EphemeralDatabase;
use sea_orm::{ActiveModelTrait, Set};
use uuid::Uuid;

async fn seed_org(conn: &sea_orm::DatabaseConnection, id: Uuid, name: &str) {
    ActiveModel {
        id: Set(id),
        name: Set(name.to_owned()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org");
}

fn read_all_ability() -> Arc<Ability> {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, Entity);
    Arc::new(b.build())
}

#[tokio::test]
async fn list_returns_seeded_orgs_for_an_unrestricted_reader() {
    let db = EphemeralDatabase::create::<db::Migrator>()
        .await
        .expect("ephemeral database");
    let org_id = Uuid::now_v7();
    seed_org(db.connection().as_ref(), org_id, "Acme").await;

    with_request_executor(Executor::Pool(db.connection()), async {
        with_ability(read_all_ability(), async {
            let rows = OrgsService::default().list().await.expect("list succeeds");
            assert!(rows.iter().any(|row| row.id == org_id && row.name == "Acme"));
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn access_hides_out_of_scope_orgs() {
    let db = EphemeralDatabase::create::<db::Migrator>()
        .await
        .expect("ephemeral database");
    let allowed = Uuid::now_v7();
    let blocked = Uuid::now_v7();
    seed_org(db.connection().as_ref(), allowed, "Allowed").await;
    seed_org(db.connection().as_ref(), blocked, "Blocked").await;

    let ability = Arc::new({
        let allowed = allowed;
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, Entity)
            .when(move |p| p.eq(Column::Id, allowed));
        b.build()
    });

    with_request_executor(Executor::Pool(db.connection()), async {
        with_ability(ability, async {
            let service = OrgsService::default();
            assert!(matches!(
                service.access(Action::Read, allowed).await.expect("allowed"),
                Access::Found(_),
            ));
            assert!(matches!(
                service.access(Action::Read, blocked).await.expect("blocked"),
                Access::Denied,
            ));
        })
        .await;
    })
    .await;
}
