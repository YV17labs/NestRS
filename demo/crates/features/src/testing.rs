use nest_rs::authn::{JwtOptions, JwtService};
use nest_rs::config::Config;
use nest_rs::core::Container;
use nest_rs::database::OutboxStore;
use nest_rs::redis::{RedisConfig, RedisConnection};
use nest_rs::storage::Storage;
use nest_rs::testing::url_on;
use serde::Serialize;
use uuid::Uuid;

use crate::audio::{AudioService, Transcodes};
use crate::{Claims, Role, Subject};

pub const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
pub const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

pub const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";

pub const AUDIENCE: &str = "http://localhost:3003";

pub const ISSUER: &str = "http://localhost:3001";

pub const SUITES_BUCKET: &str = "nestrs-test";

pub const CLIENT_ID: &str = "demo-service";

pub fn client() -> Subject {
    Subject::Client(CLIENT_ID.into())
}

pub fn user() -> Subject {
    Subject::User(Uuid::now_v7())
}

fn issuer_named(name: &str) -> JwtService {
    let mut options = JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY);
    options.issuer = Some(name.into());
    JwtService::new(options).expect("the dev keypair parses")
}

fn issuer() -> JwtService {
    issuer_named(ISSUER)
}

pub fn signed(claims: &impl Serialize) -> String {
    signed_for(AUDIENCE, claims)
}

pub fn signed_for(resource: &str, claims: &impl Serialize) -> String {
    issuer()
        .sign_for(resource, claims)
        .expect("sign the test token")
}

pub fn signed_by(issuer: &str, claims: &impl Serialize) -> String {
    issuer_named(issuer)
        .sign_for(AUDIENCE, claims)
        .expect("sign the test token")
}

pub fn token(org_id: Uuid, roles: Vec<Role>, sub: Subject) -> String {
    token_with_scopes(org_id, roles, sub, crate::authz::constants::all())
}

pub fn claims(org_id: Uuid, roles: Vec<Role>, sub: Subject, scopes: Vec<String>) -> Claims {
    let client_id = sub.client_id().map(str::to_owned);
    Claims {
        sub,
        client_id,
        org_id,
        roles,
        scopes,
        exp: issuer().expiry(),
    }
}

pub fn token_with_scopes(
    org_id: Uuid,
    roles: Vec<Role>,
    sub: Subject,
    scopes: Vec<String>,
) -> String {
    signed(&claims(org_id, roles, sub, scopes))
}

pub fn token_for(org_id: &str, role: &str, sub: Subject) -> String {
    let roles = match role {
        "admin" => vec![Role::Admin],
        _ => vec![Role::User],
    };
    token(Uuid::parse_str(org_id).expect("valid org uuid"), roles, sub)
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

pub async fn arm_outbox(container: &Container) {
    container
        .get_dyn::<dyn OutboxStore>()
        .expect("SeaOrmDatabaseModule keeps the app's outbox")
        .arm()
        .await
        .expect("the outbox arms over the migrated database");
}

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
