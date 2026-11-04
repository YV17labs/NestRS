//! The overlay `#[config]` writes: every field read under its own name, by the
//! reader its type chooses, over the base pinned in code or the default.

use std::borrow::Cow;
use std::time::Duration;

use nest_rs_config::__private::validator::Validate as _;
use nest_rs_config::{
    Bound, Config, ConfigError, ConfigService, CountBounds, DurationBounds, Field, Floor,
    FromSetting, Result, Surface, config, var_name,
};
use nest_rs_core::Secret;

const WINDOW: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(1, "a shorter window counts nothing")),
    Bound::new(3600, "past an hour it is a unit slip"),
);

const CEILING: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(1, "a shorter ceiling ends every firing")),
    Bound::new(60_000, "past a minute it is a unit slip"),
);

const KILNS: CountBounds = CountBounds::new(
    Bound::new(1, "a firing needs a kiln"),
    Bound::new(64, "a controller drives no more"),
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

/// The base a reader of its own falls back to, so the test sees it ran; a
/// pinned value past a thousand is refused naming the field it was set through.
fn doubled(env: &ConfigService, field: Field, base: u32) -> Result<u32> {
    match env.parse::<u32>(field.key())? {
        Some(read) => Ok(read * 2),
        None if base > 1000 => Err(ConfigError::parse(
            env.var_name(field.key()),
            format!("`{}` set in code is past a thousand", field.path()),
        )),
        None => Ok(base),
    }
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
    #[config(bounds = WINDOW)]
    cooldown: Option<Duration>,
    #[config(bounds = CEILING)]
    ceiling: Option<Duration>,
    #[config(bounds = KILNS)]
    kilns: usize,
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
            cooldown: Some(Duration::from_secs(5)),
            ceiling: Some(Duration::from_millis(500)),
            kilns: 2,
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
            ("COOLDOWN_SECS", "7"),
            ("CEILING_MS", "250"),
            ("KILNS", "4"),
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
    assert_eq!(read.cooldown, Some(Duration::from_secs(7)));
    assert_eq!(read.ceiling, Some(Duration::from_millis(250)));
    assert_eq!(read.kilns, 4);
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
            && refused.contains("must be at least 1 second —"),
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

/// One range bounds two fields: each reads its own key, and off follows from
/// the field's type alone — the `Option` turns off at `0`, the `Duration`
/// holds `0` to the floor.
#[test]
fn off_follows_from_the_fields_type_whatever_range_it_shares() {
    let read_one = |vars: &[(&str, &str)]| read(vars, KilnConfig::default());
    assert_eq!(
        read_one(&[("COOLDOWN_SECS", "0")])
            .expect("`0` turns the option off")
            .cooldown,
        None
    );
    let below = read_one(&[("COOLDOWN_SECS", "3601")])
        .expect_err("the shared ceiling holds")
        .to_string();
    assert!(
        below.contains(&var_name("overlay_kiln", "COOLDOWN_SECS"))
            && below.contains("must be at most 3600 seconds, or 0 to turn it off"),
        "{below}"
    );
    let pinned = read(
        &[],
        KilnConfig {
            cooldown: Some(Duration::ZERO),
            ..KilnConfig::default()
        },
    )
    .expect_err("off in code is `None`")
    .to_string();
    assert!(
        pinned.contains("`KilnConfig::cooldown` set in code is 0ns")
            && pinned.contains("`None` turns it off"),
        "{pinned}"
    );
    let window = read_one(&[("WINDOW_SECS", "0")])
        .expect_err("a `Duration` has no off")
        .to_string();
    assert!(!window.contains("turn it off"), "{window}");
}

/// The derive binds each range to the key and the path it read off the field,
/// once, for every site that names the variable or holds a value set in code.
#[test]
fn a_bounded_field_names_its_key_and_path_once() {
    assert_eq!(KilnConfig::WINDOW.key(), "WINDOW_SECS");
    assert_eq!(KilnConfig::COOLDOWN.key(), "COOLDOWN_SECS");
    assert_eq!(KilnConfig::CEILING.key(), "CEILING_MS");
    assert_eq!(KilnConfig::KILNS.key(), "KILNS");
    assert_eq!(KilnConfig::CEILING.field().path(), "KilnConfig::ceiling");
    assert_eq!(
        KilnConfig::CEILING.var("overlay_kiln"),
        var_name("overlay_kiln", "CEILING_MS")
    );
    let held = KilnConfig::WINDOW
        .check("overlay_kiln", Duration::from_secs(3601))
        .expect_err("a value built in code is held to the range")
        .to_string();
    assert!(
        held.contains(&var_name("overlay_kiln", "WINDOW_SECS"))
            && held.contains("`KilnConfig::window` set in code is 3601s")
            && !held.contains("is not set"),
        "{held}"
    );
    assert_eq!(
        KilnConfig::COOLDOWN
            .check("overlay_kiln", None)
            .expect("off"),
        None
    );
}

/// A count reads its range from either side, refused naming its variable and
/// the field it was pinned through, never clamped.
#[test]
fn a_count_is_held_to_its_range_from_either_side() {
    for (value, expected) in [
        ("0", "must be at least 1 — a firing needs a kiln"),
        ("65", "must be at most 64 — a controller drives no more"),
    ] {
        let refused = read(&[("KILNS", value)], KilnConfig::default())
            .expect_err("outside the range")
            .to_string();
        assert!(
            refused.contains(&var_name("overlay_kiln", "KILNS")) && refused.contains(expected),
            "{refused}"
        );
    }
    let pinned = read(
        &[],
        KilnConfig {
            kilns: 0,
            ..KilnConfig::default()
        },
    )
    .expect_err("a pin is held to the range the variable is")
    .to_string();
    assert!(
        pinned.contains(
            "is not set, and `KilnConfig::kilns` set in code is 0, below the 1 it must \
             be at least — a firing needs a kiln"
        ),
        "{pinned}"
    );
}

/// A field's own reader is handed its key and the path it is pinned through,
/// bound once by the derive for every site that refuses a value set in code.
#[test]
fn a_reader_of_its_own_is_handed_its_field() {
    assert_eq!(KilnConfig::HEAT.key(), "HEAT");
    assert_eq!(KilnConfig::HEAT.path(), "KilnConfig::heat");
    let refused = read(
        &[],
        KilnConfig {
            heat: 1200,
            ..KilnConfig::default()
        },
    )
    .expect_err("past a thousand in code")
    .to_string();
    assert!(
        refused.contains(&var_name("overlay_kiln", "HEAT"))
            && refused.contains("`KilnConfig::heat` set in code is past a thousand"),
        "{refused}"
    );
}

/// A count unlimited in code is `None`: a pinned `Some(0)` is refused under
/// the variable that would override it, which still outranks the pin.
#[test]
fn a_count_pinned_at_zero_is_refused_and_none_is_unlimited() {
    let zero = KilnConfig {
        batches: Some(0),
        ..KilnConfig::default()
    };
    let refused = read(&[], zero.clone()).expect_err("unlimited in code is `None`");
    assert!(
        matches!(&refused, ConfigError::Parse { var, .. } if *var == var_name("overlay_kiln", "BATCHES")),
        "{refused}"
    );
    assert_eq!(
        read(&[("BATCHES", "3")], zero)
            .expect("the variable outranks the pin")
            .batches,
        Some(3)
    );
    let unlimited = KilnConfig {
        batches: None,
        ..KilnConfig::default()
    };
    assert_eq!(read(&[], unlimited).expect("unlimited").batches, None);
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

/// A firing's limits, read whole as JSON and validated inside.
#[derive(
    Clone, Debug, Default, serde::Deserialize, ::nest_rs_config::__private::validator::Validate,
)]
#[validate(crate = ::nest_rs_config::__private::validator)]
struct Limits {
    #[validate(range(max = 10))]
    max: u32,
}

#[config(namespace = "overlay_firing")]
#[derive(Clone, Debug, Default)]
struct FiringConfig {
    #[config(json)]
    #[validate(nested)]
    limits: Limits,
}

#[test]
fn a_failure_inside_a_json_field_names_the_one_variable_that_sets_it() {
    let refused = FiringConfig {
        limits: Limits { max: 99 },
    }
    .check()
    .expect_err("a max past ten is refused")
    .to_string();
    assert!(
        refused.contains(&format!(
            "  - limits.max ({}): range",
            var_name("overlay_firing", "LIMITS")
        )),
        "{refused}"
    );
}

#[config(namespace = "overlay_surface")]
#[derive(Clone, Debug, Default)]
struct SurfaceConfig {
    samples: Surface,
}

/// A `Surface` field is read by its type alone: a boolean is written, unset is
/// the base.
#[test]
fn a_surface_field_reads_a_boolean_over_its_base() {
    let read = |vars: &[(&'static str, &'static str)]| {
        SurfaceConfig::from_env(
            &ConfigService::with_vars("overlay_surface", vars.iter().copied()),
            SurfaceConfig::default(),
        )
        .expect("reads")
        .samples
    };
    assert_eq!(read(&[]), Surface::ByProfile);
    assert_eq!(read(&[("SAMPLES", "true")]), Surface::Open);
    assert_eq!(read(&[("SAMPLES", "false")]), Surface::Closed);
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

#[config(namespace = "overlay_upstream")]
#[derive(Clone, Debug, Default)]
struct UpstreamConfig {
    upstream_tls: nest_rs_config::ClientTls,
}

/// A TLS client's checks name `TLS_CA_CERT`, `TLS_CERT` and `TLS_KEY`, so a
/// field of another name, whose variables they would misname, is refused at
/// the read.
#[test]
fn a_tls_client_under_a_field_not_named_tls_is_refused() {
    let refused = UpstreamConfig::from_env(
        &ConfigService::with_vars("overlay_upstream", []),
        UpstreamConfig::default(),
    )
    .expect_err("a TLS client is read under `tls`");
    assert!(
        matches!(&refused, ConfigError::MisnamedField { var, .. } if *var == var_name("overlay_upstream", "UPSTREAM_TLS")),
        "{refused:?}"
    );
}
