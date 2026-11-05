use std::sync::Arc;

use nest_rs::config::Config;
use nest_rs::core::Container;
use nest_rs::redis::{RedisConfig, RedisConnection};
use nest_rs::storage::Storage;
use nest_rs::testing::url_on;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
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
