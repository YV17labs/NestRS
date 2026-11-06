use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use nest_rs::config::{Config, Environment};
use nest_rs::core::Container;
use nest_rs::http::header;
use nest_rs::redis::{RedisConfig, RedisConnection};
use nest_rs::storage::Storage;
use nest_rs::testing::{LogCapture, TestApp, url_on, wait_for};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, Set, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::audio::{AudioService, Transcodes};
use crate::orgs::ActiveModel as OrgActive;
use crate::users::{ActiveModel as UserActive, UserRole};
use crate::{Claims, Role, Subject};

pub const ORG_ID: Uuid = Uuid::from_u128(0x018f_0000_0000_7000_8000_0000_0000_0000);

pub const API: &str = "http://localhost:3002";

pub const ASSISTANT: &str = "http://localhost:3003";

pub const LIVE: &str = "http://localhost:3004";

pub const SUITES_BUCKET: &str = "nestrs-test";

pub const CLIENT_ID: &str = "demo-service";

pub fn client() -> Subject {
    Subject::Client(CLIENT_ID.into())
}

pub fn user() -> Subject {
    Subject::User(Uuid::now_v7())
}

pub fn claims(org_id: Uuid, role: Role, sub: Subject) -> Claims {
    Claims {
        client_id: sub.client_id().map(str::to_owned),
        sub,
        org_id,
        roles: vec![role],
        scopes: crate::authz::constants::all(),
    }
}

pub fn with_claim(claims: Claims, claim: &str, value: &str) -> serde_json::Value {
    let mut set = serde_json::to_value(claims).expect("the claims serialize");
    set[claim] = value.into();
    set
}

pub fn connection(container: &Container) -> Arc<DatabaseConnection> {
    container
        .get::<DatabaseConnection>()
        .expect("the app's pool is its throwaway database")
}

#[track_caller]
pub fn assert_passes_under_profile(test: &str, profile: Option<&str>) {
    let mut probe = Command::new(std::env::current_exe().expect("the suite's own binary"));
    probe.args(["--exact", test]);
    match profile {
        Some(profile) => probe.env(Environment::var_name(), profile),
        None => probe.env_remove(Environment::var_name()),
    };
    let ran = probe
        .output()
        .expect("the probe runs in a process of its own");
    let said = String::from_utf8_lossy(&ran.stdout);
    assert!(
        ran.status.success() && said.contains("1 passed"),
        "{test} under {profile:?}: {said}{}",
        String::from_utf8_lossy(&ran.stderr)
    );
}

pub async fn assert_a_probe_reads_no_credential(app: &TestApp) {
    let logs = LogCapture::install();
    for probe in ["/health/live", "/health/ready"] {
        app.http()
            .get(probe)
            .header(header::AUTHORIZATION, "Bearer not-a-token")
            .send()
            .await
            .assert_status_is_ok();
    }
    let authn: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| event.target == nest_rs::authn::TARGET)
        .collect();
    assert!(authn.is_empty(), "{authn:?}");
}

pub async fn seed_org(conn: &DatabaseConnection, name: &str) -> Uuid {
    OrgActive {
        id: Set(Uuid::now_v7()),
        name: Set(name.into()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org")
    .id
}

pub async fn seed_user(
    conn: &DatabaseConnection,
    org_id: Uuid,
    name: &str,
    email: &str,
    role: UserRole,
) -> Uuid {
    UserActive {
        id: Set(Uuid::now_v7()),
        org_id: Set(org_id),
        name: Set(name.into()),
        email: Set(email.into()),
        role: Set(role),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed user")
    .id
}

pub async fn cancelling_reads_of<T>(
    conn: &DatabaseConnection,
    table: &str,
    read: impl Future<Output = T>,
) -> T {
    let lock = conn.begin().await.expect("a transaction to hold the lock");
    lock.execute_unprepared(&format!("LOCK TABLE \"{table}\" IN ACCESS EXCLUSIVE MODE"))
        .await
        .expect("the table locks");
    let cancelling = wait_for(Duration::from_secs(10), || async {
        conn.query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(pg_cancel_backend(pid)) AS cancelled FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        ))
        .await
        .expect("the waiting statements are read")
        .expect("a count is one row")
        .try_get::<i64>("", "cancelled")
        .expect("a count")
            > 0
    });
    let (answer, ()) = tokio::join!(read, cancelling);
    lock.rollback().await.expect("the lock is released");
    answer
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedisDatabase {
    WorkerRuns = 2,
    AudioUpload = 3,
    AudioMultipart = 4,
    PostsNotify = 5,
    AudioSchedule = 6,
    PostsRollback = 7,
}

impl RedisDatabase {
    pub const SUITES: u8 = 1;
    const OWN: std::ops::RangeInclusive<u8> = 2..=7;
    pub const TRANSCODES: u8 = 8;

    pub fn config(self) -> RedisConfig {
        let index = self as u8;
        assert!(
            Self::OWN.contains(&index),
            "{self:?} takes database {index}, outside the demo's own {:?}",
            Self::OWN,
        );
        nest_rs::testing::load_project_env();
        let suites = RedisConfig::load().expect("the suites' Redis config parses");
        assert!(
            Self::held(suites.url.expose_secret()) == Some(Self::SUITES),
            "the e2e suites' Redis URL must name database {}, the one nothing drains",
            Self::SUITES,
        );
        RedisConfig {
            url: url_on(suites.url.expose_secret(), index).into(),
            ..suites
        }
    }

    pub fn held(url: &str) -> Option<u8> {
        (Self::SUITES..=Self::TRANSCODES).find(|&index| url_on(url, index) == url)
    }
}

const _: () = assert!(
    RedisDatabase::SUITES < *RedisDatabase::OWN.start()
        && *RedisDatabase::OWN.end() < RedisDatabase::TRANSCODES
);

pub async fn delete_transcode(container: &Container, file: Uuid) {
    let storage = container
        .get::<Storage>()
        .expect("the app opens the audio feature's store");
    for key in [file.to_string(), AudioService::derived_key(file)] {
        storage
            .delete(&key)
            .await
            .expect("the store deletes what the test filed");
    }
    let transcodes = container
        .get::<RedisConnection<Transcodes>>()
        .expect("the app opens the transcodes instance");
    redis::cmd("DEL")
        .arg(AudioService::record_key(file))
        .query_async::<()>(&mut RedisConnection::clone(&transcodes))
        .await
        .expect("the transcodes instance deletes the record");
}
