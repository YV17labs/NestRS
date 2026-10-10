//! Covers `src/config.rs` — `AuthnConfig::into_options`.

use std::time::Duration;

use nest_rs_authn::{AuthError, AuthnConfig, JwtKey, JwtService};

// HS256 secrets must clear the 32-byte (256-bit) floor.
const STRONG_SECRET: &str = "this-is-a-32-byte-test-secret!!!";

#[test]
fn into_options_selects_hmac_from_secret() {
    let options = AuthnConfig {
        secret: Some(STRONG_SECRET.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(matches!(options.key, JwtKey::Hmac(_)));
}

/// The value is judged once, by the constructor every path reaches: a config
/// hands a short secret over as a key, and the service refuses it.
#[test]
fn a_short_hmac_secret_from_config_is_refused_by_the_service() {
    let options = AuthnConfig {
        secret: Some("too-short".into()),
        ..Default::default()
    }
    .into_options()
    .expect("a secret alone makes an HMAC key");
    assert!(matches!(
        nest_rs_authn::JwtService::new(options),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn into_options_selects_eddsa_from_key_pair() {
    let options = AuthnConfig {
        private_key: Some(crate::DEV_PRIVATE_KEY.into()),
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(matches!(options.key, JwtKey::Pem { .. }));
    JwtService::new(options).expect("EdDSA service builds");
}

#[test]
fn into_options_verify_only_from_public_key() {
    let options = AuthnConfig {
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(matches!(
        options.key,
        JwtKey::Pem {
            private_pem: None,
            ..
        }
    ));
}

#[test]
fn into_options_private_key_without_public_fails() {
    assert!(matches!(
        AuthnConfig {
            private_key: Some("pem".into()),
            ..Default::default()
        }
        .into_options(),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn into_options_without_any_key_fails() {
    assert!(matches!(
        AuthnConfig::default().into_options(),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn leeway_and_audience_are_applied_from_config() {
    let options = AuthnConfig {
        secret: Some(STRONG_SECRET.into()),
        leeway_secs: Some(45),
        audience: Some("api".into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert_eq!(options.leeway, Duration::from_secs(45));
    assert_eq!(options.audience.as_deref(), Some("api"));
}

/// A secret beside a whole EdDSA pair is refused, naming the three settings.
#[test]
fn a_secret_beside_an_eddsa_pair_is_refused_naming_all_three() {
    use nest_rs_config::{Namespaced, var_name};

    let refused = AuthnConfig {
        secret: Some(STRONG_SECRET.into()),
        private_key: Some(crate::DEV_PRIVATE_KEY.into()),
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options();
    let Err(AuthError::Failed(message)) = refused else {
        panic!("a secret beside a whole pair must be refused, not ignored")
    };
    for key in ["SECRET", "PRIVATE_KEY", "PUBLIC_KEY"] {
        assert!(
            message.contains(&var_name(AuthnConfig::NAMESPACE, key)),
            "names {key}, built rather than spelled: {message}"
        );
    }
}

#[test]
fn the_audience_opt_out_is_off_by_default_and_carries_through() {
    // Absence of an audience is not absence of the RFC 7519 §4.1.3 check.
    let default = AuthnConfig {
        secret: Some(STRONG_SECRET.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(
        !default.allow_any_audience,
        "a bare config still applies the audience clause",
    );

    let opted_out = AuthnConfig {
        secret: Some(STRONG_SECRET.into()),
        allow_any_audience: true,
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(opted_out.allow_any_audience);
}

/// A private key beside a shared secret is refused as two signing modes, naming
/// exactly the two settings that are set.
#[test]
fn a_private_key_beside_a_secret_is_refused_naming_both() {
    use nest_rs_config::{Namespaced, var_name};

    let refused = AuthnConfig {
        secret: Some(STRONG_SECRET.into()),
        private_key: Some(crate::DEV_PRIVATE_KEY.into()),
        ..Default::default()
    }
    .into_options();
    let Err(AuthError::Failed(message)) = refused else {
        panic!("a private key beside a secret must be refused")
    };
    assert!(
        message.contains(&var_name(AuthnConfig::NAMESPACE, "SECRET"))
            && message.contains(&var_name(AuthnConfig::NAMESPACE, "PRIVATE_KEY")),
        "{message}"
    );
    assert!(
        !message.contains("PUBLIC_KEY"),
        "names only what is set: {message}"
    );
}

/// `into_options` cannot tell a secret the environment set from one pinned in
/// code, so the refusal names both places and its remedy is removal from
/// wherever it was set — never "unset" a variable that may not exist.
#[test]
fn a_secret_beside_keys_is_named_for_the_environment_and_the_pin_alike() {
    let refused = AuthnConfig {
        secret: Some(STRONG_SECRET.into()),
        private_key: Some(crate::DEV_PRIVATE_KEY.into()),
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options();
    let Err(AuthError::Failed(message)) = refused else {
        panic!("a secret beside keys must be refused")
    };
    assert!(
        message.contains("`secret` in an AuthnConfig or JwtOptions built in code"),
        "{message}"
    );
    assert!(
        message.contains("`public_key` in an AuthnConfig or JwtOptions built in code"),
        "{message}"
    );
    assert!(
        message.contains("remove the secret from wherever it was set"),
        "{message}"
    );
    assert!(!message.contains("unset the secret"), "{message}");
}

/// A secret is a signing mode whatever its value, so a blank one beside a key
/// is the two-modes refusal — not a message about the secret's length that
/// leaves the key unmentioned.
#[test]
fn a_blank_secret_beside_a_public_key_is_refused_as_two_modes() {
    let refused = AuthnConfig {
        secret: Some("   ".into()),
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options();
    let Err(AuthError::Failed(message)) = refused else {
        panic!("a blank secret beside a public key must be refused")
    };
    assert!(message.contains("PUBLIC_KEY"), "{message}");
    assert!(!message.contains("must not be empty"), "{message}");
}

/// Key material that does not parse is refused by `JwtService::new`, the one site
/// every path reaches, naming the setting — not as a bare `InvalidKeyFormat`.
#[test]
fn eddsa_key_material_that_does_not_parse_names_its_setting() {
    use nest_rs_config::{Namespaced, var_name};

    let public_setting = var_name(AuthnConfig::NAMESPACE, "PUBLIC_KEY_FILE");
    for (label, public) in [
        ("an empty file", ""),
        ("whitespace", "  \n"),
        ("a private key in its place", crate::DEV_PRIVATE_KEY),
    ] {
        let refused = AuthnConfig {
            public_key: Some(public.into()),
            ..Default::default()
        }
        .into_options()
        .and_then(JwtService::new);
        let Err(AuthError::Failed(message)) = refused else {
            panic!("{label} is not a public key")
        };
        assert!(message.contains(&public_setting), "{label}: {message}");
    }

    let refused = AuthnConfig {
        private_key: Some(
            "-----BEGIN PRIVATE KEY-----\nnot base64\n-----END PRIVATE KEY-----\n".into(),
        ),
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options()
    .and_then(JwtService::new);
    let Err(AuthError::Failed(message)) = refused else {
        panic!("a private key that does not parse is refused")
    };
    assert!(
        message.contains(&var_name(AuthnConfig::NAMESPACE, "PRIVATE_KEY_FILE")),
        "{message}"
    );
}

/// A public key beside a shared secret is refused, naming the two settings that
/// are set and not the private key.
#[test]
fn a_public_key_beside_a_secret_is_refused_naming_both() {
    let refused = AuthnConfig {
        secret: Some(STRONG_SECRET.into()),
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options();
    let Err(AuthError::Failed(message)) = refused else {
        panic!("a public key beside a secret must be refused")
    };
    assert!(
        message.contains("SECRET") && message.contains("PUBLIC_KEY"),
        "{message}"
    );
    assert!(
        !message.contains("PRIVATE_KEY"),
        "names only what is set: {message}"
    );
}

/// Each key is read variable by variable like any other field, so a secret the
/// deployment sets and a pair a committed `.env` still carries arrive together
/// — and the combination is refused, never settled by choosing a tier.
#[test]
fn a_secret_and_keys_from_different_tiers_arrive_together_and_are_refused() {
    use std::collections::HashMap;
    use std::sync::Arc;

    use nest_rs_config::{Config, ConfigService, ConfigSource, Namespaced, var_name};

    struct Tiers {
        deployment: HashMap<String, String>,
        cascade: HashMap<String, String>,
    }
    impl ConfigSource for Tiers {
        fn get(&self, var: &str) -> Option<String> {
            self.deployment
                .get(var)
                .or_else(|| self.cascade.get(var))
                .cloned()
        }
        fn get_from_deployment(&self, var: &str) -> Option<String> {
            self.deployment.get(var).cloned()
        }
    }
    fn named(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (var_name(AuthnConfig::NAMESPACE, key), (*value).to_owned()))
            .collect()
    }
    fn reader(deployment: &[(&str, &str)], cascade: &[(&str, &str)]) -> ConfigService {
        ConfigService::with_source(
            AuthnConfig::NAMESPACE,
            Arc::new(Tiers {
                deployment: named(deployment),
                cascade: named(cascade),
            }),
        )
    }
    let committed_pair = [
        ("PRIVATE_KEY", crate::DEV_PRIVATE_KEY),
        ("PUBLIC_KEY", crate::DEV_PUBLIC_KEY),
    ];

    let secret_var = var_name(AuthnConfig::NAMESPACE, "SECRET");

    let config = AuthnConfig::from_env(
        &reader(&[("SECRET", STRONG_SECRET)], &committed_pair),
        AuthnConfig::default(),
    )
    .expect("from_env");
    assert!(
        config.secret.is_some() && config.private_key.is_some() && config.public_key.is_some(),
        "each variable is read on its own, so all three arrive"
    );
    let Err(AuthError::Failed(message)) = config.into_options() else {
        panic!("a deployment's secret beside a committed pair must be refused, not chosen")
    };
    assert!(message.contains(&secret_var), "{message}");

    let pinned = AuthnConfig {
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    };
    let config = AuthnConfig::from_env(
        &reader(&[("SECRET", STRONG_SECRET)], &[]).over_pinned(),
        pinned,
    )
    .expect("from_env");
    assert!(
        config.public_key.is_some() && config.secret.is_some(),
        "a deployment's secret does not erase a key pinned in code"
    );
    assert!(
        config.into_options().is_err(),
        "and the two together are refused"
    );
}

/// The EdDSA keys take the `_FILE` form every PEM certificate and key in the
/// framework takes, so a key mounted as a file configures the same field as one
/// injected as a variable.
#[test]
fn the_eddsa_keys_are_read_from_the_files_their_file_variables_name() {
    use nest_rs_config::{Config, ConfigService, Namespaced};

    let dir = std::env::temp_dir().join(format!("nest-rs-authn-keys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let private = dir.join("jwt.pem");
    let public = dir.join("jwt.pub");
    std::fs::write(&private, crate::DEV_PRIVATE_KEY).expect("write the private key");
    std::fs::write(&public, crate::DEV_PUBLIC_KEY).expect("write the public key");
    let env = ConfigService::with_vars(
        AuthnConfig::NAMESPACE,
        [
            ("PRIVATE_KEY_FILE", private.to_str().expect("a UTF-8 path")),
            ("PUBLIC_KEY_FILE", public.to_str().expect("a UTF-8 path")),
        ],
    );
    let config = AuthnConfig::from_env(&env, AuthnConfig::default());
    let _ = std::fs::remove_dir_all(&dir);

    let config = config.expect("from_env");
    assert_eq!(config.private_key.as_deref(), Some(crate::DEV_PRIVATE_KEY));
    assert_eq!(config.public_key.as_deref(), Some(crate::DEV_PUBLIC_KEY));
}

#[test]
fn the_audience_opt_out_reads_its_env_flag() {
    use nest_rs_config::{Config, ConfigService, Namespaced};

    // The namespace comes off the config type, not a literal, so the fixture
    // cannot mean a variable the reader does not.
    let env = ConfigService::with_vars(AuthnConfig::NAMESPACE, [("ALLOW_ANY_AUDIENCE", "true")]);
    let config = AuthnConfig::from_env(
        &env,
        AuthnConfig {
            secret: Some(STRONG_SECRET.into()),
            ..Default::default()
        },
    )
    .expect("from_env");
    assert!(
        config.allow_any_audience,
        "the deployment can state the opt-out, and only by stating it",
    );
}

/// RFC 7518 §3.2: an HMAC key is at least its hash's size, so a secret that is
/// long enough for HS256 is refused for HS384 and HS512.
#[test]
fn a_secret_is_held_to_the_size_of_its_algorithms_hash() {
    use jsonwebtoken::Algorithm;

    for (algorithm, minimum) in [(Algorithm::HS384, 48), (Algorithm::HS512, 64)] {
        let mut short = nest_rs_authn::JwtOptions::new(STRONG_SECRET);
        short.algorithms = vec![algorithm];
        let Err(AuthError::Failed(message)) = JwtService::new(short) else {
            panic!("a 32-byte secret is too short for {algorithm:?}")
        };
        assert!(message.contains(&format!("{minimum} bytes")), "{message}");

        let mut long = nest_rs_authn::JwtOptions::new("x".repeat(minimum));
        long.algorithms = vec![algorithm];
        JwtService::new(long).expect("a secret of the hash's size is accepted");
    }
}

/// `EXPIRES_IN_SECS` and `LEEWAY_SECS` are refused past their range, naming the
/// variable, from the environment and from code alike; each end is accepted.
#[test]
fn a_lifetime_or_a_leeway_outside_its_range_is_refused_naming_the_variable() {
    use nest_rs_config::{Config, ConfigService, var_name};

    let read = |vars: &[(&str, &str)], base: AuthnConfig| {
        AuthnConfig::from_env(
            &ConfigService::with_vars("authn", vars.iter().copied()),
            base,
        )
    };
    let max = u64::MAX.to_string();
    for (key, value) in [
        ("EXPIRES_IN_SECS", "0"),
        ("EXPIRES_IN_SECS", "2592001"),
        ("EXPIRES_IN_SECS", max.as_str()),
        ("LEEWAY_SECS", "301"),
        ("LEEWAY_SECS", max.as_str()),
    ] {
        let refused = read(&[(key, value)], AuthnConfig::default())
            .err()
            .unwrap_or_else(|| panic!("{key}={value} must fail the boot"))
            .to_string();
        assert!(refused.contains(&var_name("authn", key)), "{refused}");
    }
    for (pinned, key) in [
        (
            AuthnConfig {
                expires_in_secs: Some(u64::MAX),
                ..Default::default()
            },
            "EXPIRES_IN_SECS",
        ),
        (
            AuthnConfig {
                leeway_secs: Some(u64::MAX),
                ..Default::default()
            },
            "LEEWAY_SECS",
        ),
    ] {
        let refused = read(&[], pinned)
            .err()
            .unwrap_or_else(|| panic!("a pinned {key} out of range must fail the boot"))
            .to_string();
        assert!(
            refused.contains(&var_name("authn", key)) && refused.contains("set in code"),
            "{refused}"
        );
    }
    let edges = read(
        &[("EXPIRES_IN_SECS", "2592000"), ("LEEWAY_SECS", "0")],
        AuthnConfig::default(),
    )
    .expect("each end of the range is inside it");
    assert_eq!(edges.expires_in_secs, Some(30 * 24 * 60 * 60));
    assert_eq!(edges.leeway_secs, Some(0));
    let unset = read(&[], AuthnConfig::default()).expect("unset keeps the defaults");
    assert_eq!((unset.expires_in_secs, unset.leeway_secs), (None, None));
}

// An external issuer's JWK Set, and the algorithms a token may be signed with.

const JWKS_URI: &str = "https://auth.example.com/api/auth/jwks";

/// A JWK Set and a key of this deployment's own are two key sources, refused
/// naming exactly the settings that are set.
#[test]
fn a_jwk_set_uri_beside_a_static_key_is_refused_naming_both() {
    use nest_rs_config::{Namespaced, var_name};

    let jwks = var_name(AuthnConfig::NAMESPACE, "JWKS_URI");
    for (beside, set, unset) in [
        (
            AuthnConfig {
                secret: Some(STRONG_SECRET.into()),
                ..Default::default()
            },
            "SECRET",
            "PUBLIC_KEY",
        ),
        (
            AuthnConfig {
                public_key: Some(crate::DEV_PUBLIC_KEY.into()),
                ..Default::default()
            },
            "PUBLIC_KEY",
            "SECRET",
        ),
    ] {
        let refused = AuthnConfig {
            jwks_uri: Some(JWKS_URI.into()),
            ..beside
        }
        .into_options();
        let Err(AuthError::Failed(message)) = refused else {
            panic!("a JWK Set beside {set} must be refused, not chosen")
        };
        assert!(message.contains(&jwks), "{message}");
        assert!(
            message.contains(&var_name(AuthnConfig::NAMESPACE, set)),
            "{message}"
        );
        assert!(
            !message.contains(&var_name(AuthnConfig::NAMESPACE, unset)),
            "names only what is set: {message}"
        );
    }
}

/// An authority names what the JWK Set endpoint's certificate chains to, so
/// set without a JWK Set it would go unused.
#[test]
fn an_authority_without_a_jwk_set_is_refused() {
    use nest_rs_config::{Namespaced, var_name};

    let refused = AuthnConfig {
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        tls: nest_rs_config::ClientTls::new(
            Some(nest_rs_config::Material {
                bytes: b"-----BEGIN CERTIFICATE-----".to_vec(),
                path: None,
            }),
            None,
        ),
        ..Default::default()
    }
    .into_options();
    let Err(AuthError::Failed(message)) = refused else {
        panic!("an authority with nothing to trust it for must be refused")
    };
    for key in ["TLS_CA_CERT", "JWKS_URI"] {
        assert!(
            message.contains(&var_name(AuthnConfig::NAMESPACE, key)),
            "{message}"
        );
    }
}

#[test]
fn nothing_configured_names_the_jwk_set_among_the_key_sources() {
    use nest_rs_config::{Namespaced, var_name};

    let Err(AuthError::Failed(message)) = AuthnConfig::default().into_options() else {
        panic!("no key must fail the boot")
    };
    assert!(
        message.contains(&var_name(AuthnConfig::NAMESPACE, "JWKS_URI")),
        "{message}"
    );
}

/// The URI and its authority come from the environment like every other key,
/// the authority through its `_FILE` spelling too.
#[test]
fn the_jwk_set_and_its_authority_are_read_from_the_environment() {
    use nest_rs_config::{Config, ConfigService, Namespaced};

    let pem = nest_rs_testing::TestAuthority::new().pem().to_owned();
    let dir = std::env::temp_dir().join(format!("nest-rs-authn-jwks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let authority = dir.join("issuer-ca.pem");
    std::fs::write(&authority, &pem).expect("write the authority");
    let env = ConfigService::with_vars(
        AuthnConfig::NAMESPACE,
        [
            ("JWKS_URI", JWKS_URI),
            (
                "TLS_CA_CERT_FILE",
                authority.to_str().expect("a UTF-8 path"),
            ),
        ],
    );
    let config = AuthnConfig::from_env(&env, AuthnConfig::default());
    let _ = std::fs::remove_dir_all(&dir);

    let options = config.expect("from_env").into_options().expect("options");
    let JwtKey::Jwks { uri, tls } = &options.key else {
        panic!("a JWK Set URI alone makes a JWK Set key")
    };
    assert_eq!(uri, JWKS_URI);
    assert_eq!(tls.authorities_pem(), pem.as_bytes());
    JwtService::new(options).expect("the authority holds a certificate");
}

/// `ALGORITHMS` reads the names RFC 7518 gives them, case and all, and refuses
/// anything else — `none` included — naming the variable. A list naming none
/// is the service's to refuse, like any list that does not fit its key.
#[test]
fn algorithms_are_read_by_their_jose_names() {
    use jsonwebtoken::Algorithm;
    use nest_rs_config::{Config, ConfigService, Namespaced, var_name};

    let read = |value: &str| {
        AuthnConfig::from_env(
            &ConfigService::with_vars(AuthnConfig::NAMESPACE, [("ALGORITHMS", value)]),
            AuthnConfig::default(),
        )
    };
    let config = read(" EdDSA, RS256 ,").expect("two algorithms");
    assert_eq!(
        config.algorithms,
        Some(vec![Algorithm::EdDSA, Algorithm::RS256])
    );
    let empty = AuthnConfig {
        jwks_uri: Some(JWKS_URI.into()),
        ..read(",").expect("a list of no names reads")
    };
    let Err(AuthError::Failed(message)) = empty.into_options().and_then(JwtService::new) else {
        panic!("a list naming no algorithm must fail the boot")
    };
    assert!(
        message.contains(&var_name(AuthnConfig::NAMESPACE, "ALGORITHMS"))
            && message.contains("names no algorithm"),
        "{message}"
    );
    for refused in ["none", "rs256", "ES512", "HS256,nope"] {
        let message = read(refused)
            .err()
            .unwrap_or_else(|| panic!("{refused:?} must fail the boot"))
            .to_string();
        assert!(
            message.contains(&var_name(AuthnConfig::NAMESPACE, "ALGORITHMS")),
            "{message}"
        );
    }
    let unset = AuthnConfig::from_env(
        &ConfigService::with_vars(AuthnConfig::NAMESPACE, [("JWKS_URI", JWKS_URI)]),
        AuthnConfig::default(),
    )
    .expect("from_env");
    assert_eq!(unset.algorithms, None, "unset keeps each key's default");
}

/// What `ALGORITHMS` names is judged against the key it is set beside.
#[test]
fn the_algorithms_set_beside_a_key_must_fit_it() {
    use jsonwebtoken::Algorithm;

    let narrowed = AuthnConfig {
        jwks_uri: Some(JWKS_URI.into()),
        algorithms: Some(vec![Algorithm::EdDSA]),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert_eq!(narrowed.algorithms, vec![Algorithm::EdDSA]);
    JwtService::new(narrowed).expect("EdDSA fits a JWK Set");

    let refused = AuthnConfig {
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        algorithms: Some(vec![Algorithm::RS256]),
        ..Default::default()
    }
    .into_options()
    .and_then(JwtService::new);
    let Err(AuthError::Failed(message)) = refused else {
        panic!("RS256 with an EdDSA key must be refused")
    };
    assert!(message.contains("algorithm cannot be used"), "{message}");
}

/// A secret signs with the one HMAC algorithm `ALGORITHMS` names, held to its
/// hash's size.
#[tokio::test]
async fn a_secret_signs_with_the_hmac_algorithm_named() {
    use jsonwebtoken::Algorithm;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct Claims {
        sub: String,
        exp: u64,
    }

    let jwt = JwtService::new(
        AuthnConfig {
            secret: Some("x".repeat(64)),
            algorithms: Some(vec![Algorithm::HS512]),
            ..Default::default()
        }
        .into_options()
        .expect("options"),
    )
    .expect("a 64-byte secret fits HS512");
    let token = jwt
        .sign(&Claims {
            sub: "ada".into(),
            exp: jwt.expiry(),
        })
        .expect("signs");
    assert_eq!(
        jsonwebtoken::decode_header(&token).expect("a header").alg,
        Algorithm::HS512
    );
    jwt.verify::<Claims>(&token).await.expect("verifies");
}
