use nest_rs::authn::{JwtOptions, JwtService};
use nest_rs::config::Config;
use nest_rs::redis::RedisConfig;
use nest_rs::testing::url_on;
use uuid::Uuid;

use crate::{Claims, Role};

pub const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
pub const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

pub const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";

pub const AUDIENCE: &str = "http://localhost:3003";

pub fn token(org_id: Uuid, roles: Vec<Role>, sub: Option<Uuid>) -> String {
    token_with_scopes(org_id, roles, sub, crate::authz::constants::all())
}

pub fn token_with_scopes(
    org_id: Uuid,
    roles: Vec<Role>,
    sub: Option<Uuid>,
    scopes: Vec<String>,
) -> String {
    let mut options = JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY);
    options.audience = Some(AUDIENCE.into());
    let jwt = JwtService::new(options).expect("the dev keypair parses");
    jwt.sign(&Claims {
        sub,
        org_id,
        roles,
        scopes,
        exp: jwt.expiry(),
    })
    .expect("sign the test token")
}

pub fn token_for(org_id: &str, role: &str, sub: Option<Uuid>) -> String {
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
    const SUITES: u8 = 1;
    const OWN: std::ops::RangeInclusive<u8> = 2..=8;

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
            url_on(&suites.url, Self::SUITES) == suites.url,
            "the e2e suites' Redis URL must name database {}, the one nothing drains",
            Self::SUITES,
        );
        RedisConfig {
            url: url_on(&suites.url, index),
            ..suites
        }
    }
}
