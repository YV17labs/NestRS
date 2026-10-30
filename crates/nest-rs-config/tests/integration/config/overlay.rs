//! The overlay `#[config]` writes: every field read under its own name, by the
//! reader its type chooses, over the base pinned in code or the default.

use std::borrow::Cow;
use std::time::Duration;

use nest_rs_config::{
    Bound, Config, ConfigError, ConfigService, DurationBounds, Floor, FromSetting, Result, config,
    var_name,
};
use nest_rs_core::Secret;

const WINDOW: DurationBounds = DurationBounds::secs(
    "WINDOW_SECS",
    "KilnConfig::window",
    Floor::Units(Bound::new(1, "a shorter window counts nothing")),
    Bound::new(3600, "past an hour it is a unit slip"),
);

const CEILING: DurationBounds = DurationBounds::millis(
    "CEILING_MS",
    "KilnConfig::ceiling",
    Floor::UnitsOrOff(Bound::new(1, "a shorter ceiling ends every firing")),
    Bound::new(60_000, "past a minute it is a unit slip"),
);

/// A value a test type of its own reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glaze {
    Matte,
    Gloss,
}

impl FromSetting for Glaze {
    fn from_setting(text: &str) -> std::result::Result<Self, Cow<'static, str>> {
        match text {
            "matte" => Ok(Self::Matte),
            "gloss" => Ok(Self::Gloss),
            _ => Err("is neither `matte` nor `gloss`".into()),
        }
    }
}

/// A shelf, as JSON.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
struct Shelf {
    name: String,
    slots: u8,
}

/// The base a reader of its own falls back to, so the test sees it ran.
fn doubled(env: &ConfigService, key: &str, base: u32) -> Result<u32> {
    Ok(env.parse::<u32>(key)?.map_or(base, |read| read * 2))
}

#[config(namespace = "overlay_kiln")]
#[derive(Clone, Debug, PartialEq)]
struct KilnConfig {
    host: String,
    port: u16,
    vented: bool,
    label: Option<String>,
    sensors: Vec<String>,
    token: Secret<String>,
    glaze: Glaze,
    r#type: Option<String>,
    #[config(bounds = WINDOW)]
    window: Duration,
    #[config(bounds = CEILING)]
    ceiling: Option<Duration>,
    #[config(json)]
    shelves: Vec<Shelf>,
    #[config(count)]
    batches: Option<usize>,
    #[config(with = doubled)]
    heat: u32,
}

impl Default for KilnConfig {
    fn default() -> Self {
        Self {
            host: "kiln.local".into(),
            port: 3000,
            vented: false,
            label: None,
            sensors: Vec::new(),
            token: Secret::default(),
            glaze: Glaze::Matte,
            r#type: None,
            window: Duration::from_secs(60),
            ceiling: Some(Duration::from_millis(500)),
            shelves: Vec::new(),
            batches: Some(8),
            heat: 900,
        }
    }
}

fn read(vars: &[(&str, &str)], base: KilnConfig) -> Result<KilnConfig> {
    KilnConfig::from_env(
        &ConfigService::with_vars("overlay_kiln", vars.iter().copied()),
        base,
    )
}

#[test]
fn every_field_reads_the_variable_its_name_spells() {
    let read = read(
        &[
            ("HOST", "kiln.example"),
            ("PORT", "8080"),
            ("VENTED", "yes"),
            ("LABEL", "east"),
            ("SENSORS", "door, floor"),
            ("TOKEN", "hunter2"),
            ("GLAZE", "gloss"),
            ("TYPE", "electric"),
            ("WINDOW_SECS", "30"),
            ("CEILING_MS", "250"),
            ("SHELVES", r#"[{"name":"top","slots":4}]"#),
            ("BATCHES", "0"),
            ("HEAT", "500"),
        ],
        KilnConfig::default(),
    )
    .expect("every variable reads");
    assert_eq!(read.host, "kiln.example");
    assert_eq!(read.port, 8080);
    assert!(read.vented);
    assert_eq!(read.label.as_deref(), Some("east"));
    assert_eq!(read.sensors, ["door", "floor"]);
    assert_eq!(read.token.expose_secret(), "hunter2");
    assert_eq!(read.glaze, Glaze::Gloss);
    assert_eq!(
        read.r#type.as_deref(),
        Some("electric"),
        "a raw ident reads its name"
    );
    assert_eq!(read.window, Duration::from_secs(30));
    assert_eq!(read.ceiling, Some(Duration::from_millis(250)));
    assert_eq!(
        read.shelves,
        [Shelf {
            name: "top".into(),
            slots: 4
        }]
    );
    assert_eq!(read.batches, None, "`0` is unlimited");
    assert_eq!(read.heat, 1000, "the field's own reader ran");
}

#[test]
fn an_unset_variable_keeps_the_field_pinned_in_code() {
    let pinned = KilnConfig {
        host: "pinned".into(),
        label: Some("pinned".into()),
        ceiling: None,
        ..KilnConfig::default()
    };
    let read = read(&[("PORT", "8080")], pinned.clone()).expect("the overlay resolves");
    assert_eq!(read.port, 8080, "the deployment outranks the pin");
    assert_eq!(
        KilnConfig {
            port: pinned.port,
            ..read
        },
        pinned,
        "every field the deployment is silent about keeps the pin"
    );
}

#[test]
fn a_duration_reads_its_bounds_and_off_is_none_alone() {
    let off = read(&[("CEILING_MS", "0")], KilnConfig::default()).expect("`0` turns it off");
    assert_eq!(off.ceiling, None);
    let refused = read(&[("WINDOW_SECS", "0")], KilnConfig::default())
        .expect_err("the window has no off")
        .to_string();
    assert!(
        refused.contains(&var_name("overlay_kiln", "WINDOW_SECS"))
            && refused.contains("must be at least 1 second"),
        "{refused}"
    );
    let pinned = read(
        &[],
        KilnConfig {
            window: Duration::ZERO,
            ..KilnConfig::default()
        },
    )
    .expect_err("a pinned zero is held to the same range")
    .to_string();
    assert!(
        pinned.contains("`KilnConfig::window` set in code is 0ns"),
        "{pinned}"
    );
}

#[test]
fn a_value_that_does_not_read_is_refused_by_its_variable_never_by_its_value() {
    for (key, value, expected) in [
        ("PORT", "hunter2", "does not parse as u16"),
        ("VENTED", "hunter2", "expected a boolean"),
        ("GLAZE", "hunter2", "is neither `matte` nor `gloss`"),
    ] {
        let refused = read(&[(key, value)], KilnConfig::default()).expect_err("refused");
        assert!(
            matches!(&refused, ConfigError::Parse { var, .. } if *var == var_name("overlay_kiln", key)),
            "{refused}"
        );
        let refused = refused.to_string();
        assert!(refused.contains(expected), "{refused}");
        assert!(!refused.contains("hunter2"), "{refused}");
    }
}

#[config(namespace = "overlay_profiled", defaults = profiled)]
#[derive(Clone, Debug, Default)]
struct ProfiledConfig {
    open: bool,
}

fn profiled() -> ProfiledConfig {
    ProfiledConfig { open: true }
}

#[test]
fn a_profile_aware_base_is_the_one_defaults_names() {
    assert!(ProfiledConfig::defaults().open);
    assert!(
        !ProfiledConfig::default().open,
        "`Default` stays the production-safe value"
    );
}

#[config(namespace = "overlay_manual", from_env = "manual")]
#[derive(Clone, Debug, Default)]
struct ManualConfig {
    shout: String,
}

impl Config for ManualConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            shout: env
                .get("WHISPER")?
                .map_or(base.shout, |whisper| whisper.to_uppercase()),
        })
    }
}

#[test]
fn a_manual_overlay_is_the_structs_own() {
    let env = ConfigService::with_vars("overlay_manual", [("WHISPER", "hush")]);
    assert_eq!(
        ManualConfig::from_env(&env, ManualConfig::default())
            .expect("read")
            .shout,
        "HUSH"
    );
}
