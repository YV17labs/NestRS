use std::process::Stdio;
use std::time::Duration;

use auth::{ClientPayload, SignInConfig};
use features::authz::constants;
use features::testing::{API, CLIENT_ID, ORG_ID};
use nest_rs::authn::AuthnConfig;
use nest_rs::config::{Namespaced, var_name};
use nest_rs::core::EnvPrefix;
use nest_rs::oauth::server::OAuthServerConfig;
use nest_rs::seaorm::SeaOrmConfig;
use nest_rs::social::{GithubSocialConfig, GoogleSocialConfig};
use nest_rs::testing::EphemeralDatabase;
use tokio::process::Command;
use tokio::time::timeout;

const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const OWN_DIRECTORY: &str = env!("CARGO_MANIFEST_DIR");
const OWN_SECRET: &str = "a-secret-this-test-names-on-the-process-alone";
const OWN_ORIGIN: &str = "https://auth.example.com";
const OWN_REDIRECT: &str = "https://auth.example.com/social/callback";

async fn production_boot_refuses(
    directory: &str,
    database: &EphemeralDatabase,
    named: &[&(String, String)],
) -> String {
    let child = Command::new(env!("CARGO_BIN_EXE_auth"))
        .current_dir(directory)
        .env_clear()
        .env(EnvPrefix::VAR, EnvPrefix::current())
        .env(var_name(SeaOrmConfig::NAMESPACE, "URL"), database.url())
        .envs(named.iter().map(|(variable, value)| (variable, value)))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("the auth binary starts");
    let ran = timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .expect("a boot missing what it must name stops")
        .expect("the auth binary's refusal is read");
    assert!(!ran.status.success(), "{directory}: the boot succeeded");
    String::from_utf8_lossy(&ran.stderr).into_owned()
}

async fn assert_it_names_its_own_or_is_refused(directory: &str) {
    let database = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("a database of the test's own");
    let key = (
        var_name(AuthnConfig::NAMESPACE, "SECRET"),
        OWN_SECRET.to_owned(),
    );
    let issuer = (
        var_name(AuthnConfig::NAMESPACE, "ISSUER"),
        OWN_ORIGIN.to_owned(),
    );
    let audience = (
        var_name(AuthnConfig::NAMESPACE, "AUDIENCE"),
        OWN_ORIGIN.to_owned(),
    );
    let clients = (
        var_name(OAuthServerConfig::<ClientPayload>::NAMESPACE, "CLIENTS"),
        serde_json::json!([{
            "client_id": CLIENT_ID,
            "client_secret": OWN_SECRET,
            "payload": { "org_id": ORG_ID, "roles": ["admin"] },
            "scopes": constants::all(),
            "resources": [API],
        }])
        .to_string(),
    );
    let resources = (
        var_name(SignInConfig::NAMESPACE, "RESOURCES"),
        API.to_owned(),
    );
    let github = (
        var_name(GithubSocialConfig::NAMESPACE, "REDIRECT_URL"),
        OWN_REDIRECT.to_owned(),
    );
    let google = (
        var_name(GoogleSocialConfig::NAMESPACE, "REDIRECT_URL"),
        OWN_REDIRECT.to_owned(),
    );

    for (named, said) in [
        (vec![], "no JWT key configured"),
        (vec![&key], issuer.0.as_str()),
        (vec![&key, &issuer, &audience], "at_least_one_client"),
        (
            vec![&key, &issuer, &audience, &clients],
            "at_least_one_resource",
        ),
        (
            vec![&key, &issuer, &audience, &clients, &resources, &github],
            "social provider `github` is linked but misconfigured",
        ),
        (
            vec![&key, &issuer, &audience, &clients, &resources, &google],
            "social provider `google` is linked but misconfigured",
        ),
    ] {
        let refused = production_boot_refuses(directory, &database, &named).await;
        assert!(refused.contains(said), "{directory}: {refused}");
    }
}

#[tokio::test]
async fn run_from_the_workspace_root_it_reads_no_committed_credential() {
    assert_it_names_its_own_or_is_refused(WORKSPACE_ROOT).await;
}

#[tokio::test]
async fn run_from_its_own_directory_it_reads_no_committed_credential() {
    assert_it_names_its_own_or_is_refused(OWN_DIRECTORY).await;
}
