//! Covers `src/config.rs` — `JwtConfig::into_options`.

use std::time::Duration;

use nest_rs_authn::{AuthError, JwtConfig, JwtKey, JwtService};

// HS256 secrets must clear the 32-byte (256-bit) floor.
const STRONG_SECRET: &str = "this-is-a-32-byte-test-secret!!!";

#[test]
fn into_options_selects_hmac_from_secret() {
    let options = JwtConfig {
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
    let options = JwtConfig {
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
    let options = JwtConfig {
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
    let options = JwtConfig {
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
        JwtConfig {
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
        JwtConfig::default().into_options(),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn leeway_and_audience_are_applied_from_config() {
    let options = JwtConfig {
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

/// A secret beside a whole EdDSA pair describes two signing modes, and choosing
/// either drops a credential the deployment set — so it is refused, naming the
/// three settings, rather than resolved in favour of the pair.
#[test]
fn a_secret_beside_an_eddsa_pair_is_refused_naming_all_three() {
    use nest_rs_config::{Namespaced, var_name};

    let refused = JwtConfig {
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
            message.contains(&var_name(JwtConfig::NAMESPACE, key)),
            "names {key}, built rather than spelled: {message}"
        );
    }
}

#[test]
fn the_audience_opt_out_is_off_by_default_and_carries_through() {
    // The config path's half of RFC 7519 §4.1.3: absence of an audience is not
    // absence of the check, and the only thing that turns it off is the named
    // field.
    let default = JwtConfig {
        secret: Some(STRONG_SECRET.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(
        !default.allow_any_audience,
        "a bare config still applies the audience clause",
    );

    let opted_out = JwtConfig {
        secret: Some(STRONG_SECRET.into()),
        allow_any_audience: true,
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(opted_out.allow_any_audience);
}

/// A private key beside a shared secret is refused first as the two signing
/// modes it is, naming exactly the two settings that are set. With the secret
/// beside it, the half used to be dropped in silence and the service minted
/// HS256 — and a refusal that named only the missing public key sent an operator
/// who set a deployment secret to look for a variable they never meant to set.
#[test]
fn a_private_key_beside_a_secret_is_refused_naming_both() {
    use nest_rs_config::{Namespaced, var_name};

    let refused = JwtConfig {
        secret: Some(STRONG_SECRET.into()),
        private_key: Some(crate::DEV_PRIVATE_KEY.into()),
        ..Default::default()
    }
    .into_options();
    let Err(AuthError::Failed(message)) = refused else {
        panic!("a private key beside a secret must be refused")
    };
    assert!(
        message.contains(&var_name(JwtConfig::NAMESPACE, "SECRET"))
            && message.contains(&var_name(JwtConfig::NAMESPACE, "PRIVATE_KEY")),
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
    let refused = JwtConfig {
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
        message.contains("`secret` in a JwtConfig or JwtOptions built in code"),
        "{message}"
    );
    assert!(
        message.contains("`public_key` in a JwtConfig or JwtOptions built in code"),
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
    let refused = JwtConfig {
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

    let public_setting = var_name(JwtConfig::NAMESPACE, "PUBLIC_KEY_FILE");
    for (label, public) in [
        ("an empty file", ""),
        ("whitespace", "  \n"),
        ("a private key in its place", crate::DEV_PRIVATE_KEY),
    ] {
        let refused = JwtConfig {
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

    let refused = JwtConfig {
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
        message.contains(&var_name(JwtConfig::NAMESPACE, "PRIVATE_KEY_FILE")),
        "{message}"
    );
}

/// A public key beside a shared secret describes an issuer and a verifier at
/// once, and either reading breaks one of them — so it is refused, naming the
/// two settings that are set and not the private key, which is not.
#[test]
fn a_public_key_beside_a_secret_is_refused_naming_both() {
    let refused = JwtConfig {
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
            .map(|(key, value)| (var_name(JwtConfig::NAMESPACE, key), (*value).to_owned()))
            .collect()
    }
    fn reader(deployment: &[(&str, &str)], cascade: &[(&str, &str)]) -> ConfigService {
        ConfigService::with_source(
            JwtConfig::NAMESPACE,
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

    let secret_var = var_name(JwtConfig::NAMESPACE, "SECRET");

    let config = JwtConfig::from_env(
        &reader(&[("SECRET", STRONG_SECRET)], &committed_pair),
        JwtConfig::default(),
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

    let pinned = JwtConfig {
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    };
    let config = JwtConfig::from_env(
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
        JwtConfig::NAMESPACE,
        [
            ("PRIVATE_KEY_FILE", private.to_str().expect("a UTF-8 path")),
            ("PUBLIC_KEY_FILE", public.to_str().expect("a UTF-8 path")),
        ],
    );
    let config = JwtConfig::from_env(&env, JwtConfig::default());
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
    let env = ConfigService::with_vars(JwtConfig::NAMESPACE, [("ALLOW_ANY_AUDIENCE", "true")]);
    let config = JwtConfig::from_env(
        &env,
        JwtConfig {
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
        short.algorithm = algorithm;
        let Err(AuthError::Failed(message)) = JwtService::new(short) else {
            panic!("a 32-byte secret is too short for {algorithm:?}")
        };
        assert!(message.contains(&format!("{minimum} bytes")), "{message}");

        let mut long = nest_rs_authn::JwtOptions::new("x".repeat(minimum));
        long.algorithm = algorithm;
        JwtService::new(long).expect("a secret of the hash's size is accepted");
    }
}
