//! [`DurationBounds`] and [`CountBounds`] — the range a setting a deployment
//! sets is held to — and [`BoundedSetting`], a range bound to the field it
//! holds: the one reader of that field, and the one sentence refusing a value
//! outside its range, read or pinned in code.
//!
//! Every range has both ends: the floor is where the thing stops working, the
//! ceiling never above what the library or kernel the value reaches accepts. A
//! range names neither its variable nor its field: `#[config(bounds = …)]`
//! reads both off the field it bounds, so one range serves every field it fits.
//! `0` is off only for an `Option<Duration>` field — `None` in code — and held
//! to the floor everywhere else.

use std::borrow::Cow;
use std::fmt;
use std::time::Duration;

use crate::error::ConfigError;
use crate::field::Field;
use crate::service::{ConfigService, var_name, var_name_or_field};

/// The whole-number unit a duration's variable is written in — the suffix its
/// key carries after the field's name, `_SECS` or `_MS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DurationUnit {
    /// Whole seconds — every `*_SECS` key.
    Seconds,
    /// Whole milliseconds — every `*_MS` key, for a bound that must be expressible
    /// inside a second.
    Millis,
}

impl DurationUnit {
    /// `count` of this unit as a [`Duration`].
    pub const fn duration(self, count: u64) -> Duration {
        match self {
            Self::Seconds => Duration::from_secs(count),
            Self::Millis => Duration::from_millis(count),
        }
    }

    /// The suffix a key read in this unit carries — `_SECS` or `_MS`.
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Seconds => "_SECS",
            Self::Millis => "_MS",
        }
    }

    fn name(self, count: u64) -> &'static str {
        match (self, count) {
            (Self::Seconds, 1) => "second",
            (Self::Seconds, _) => "seconds",
            (Self::Millis, 1) => "millisecond",
            (Self::Millis, _) => "milliseconds",
        }
    }
}

/// One end of a range, in its setting's unit, and why nothing past it holds —
/// the reason a refusal gives, so it is written for the operator reading the
/// boot error.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Bound {
    /// The end — a count of the [`DurationUnit`] for a duration.
    pub count: u64,
    /// Why a value past this end cannot work — never quoting the value.
    pub why: &'static str,
}

impl Bound {
    /// The end at `count`, past which nothing works for the reason `why` gives.
    pub const fn new(count: u64, why: &'static str) -> Self {
        Self { count, why }
    }
}

/// Where a duration's range starts.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Floor {
    /// At least `count` of the unit, whichever side set the value.
    Units(Bound),
    /// Longer than nothing: the variable is held to one whole unit, and a value
    /// set in code to anything above zero is taken. The reason is why zero fails.
    AboveZero(&'static str),
}

impl Floor {
    /// The least count the variable may hold.
    const fn count(self) -> u64 {
        match self {
            Self::Units(bound) => bound.count,
            Self::AboveZero(_) => 1,
        }
    }
}

/// The range one duration setting accepts, in the unit its variable is written
/// in — and nothing else: the field it bounds supplies the variable's key (its
/// name in SCREAMING_SNAKE, the unit's suffix after it) and whether `0` turns
/// it off (an `Option<Duration>`).
///
/// Built only through [`secs`](Self::secs) or [`millis`](Self::millis); a floor
/// above the ceiling does not compile in a `const`.
///
/// ```
/// use std::time::Duration;
/// use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, config};
///
/// const WINDOW: DurationBounds = DurationBounds::secs(
///     Floor::Units(Bound::new(1, "a shorter window counts nothing")),
///     Bound::new(3600, "past an hour it is a unit slip"),
/// );
///
/// #[config(namespace = "fixture")]
/// #[derive(Clone, Debug)]
/// pub struct FixtureConfig {
///     #[config(bounds = WINDOW)]
///     pub window: Duration,
///     #[config(bounds = WINDOW)]
///     pub ceiling: Option<Duration>,
/// }
/// # impl Default for FixtureConfig {
/// #     fn default() -> Self {
/// #         Self { window: Duration::from_secs(60), ceiling: None }
/// #     }
/// # }
///
/// assert_eq!(FixtureConfig::WINDOW.key(), "WINDOW_SECS");
/// let env = ConfigService::with_vars("fixture", [("WINDOW_SECS", "30"), ("CEILING_SECS", "0")]);
/// let read = FixtureConfig::from_env(&env, FixtureConfig::default())?;
/// assert_eq!(read.window, Duration::from_secs(30));
/// assert_eq!(read.ceiling, None, "`0` turns an `Option<Duration>` off");
///
/// let zero = ConfigService::with_vars("fixture", [("WINDOW_SECS", "0")]);
/// assert!(FixtureConfig::from_env(&zero, FixtureConfig::default()).is_err());
///
/// let pinned = FixtureConfig { window: Duration::ZERO, ..FixtureConfig::default() };
/// let env = ConfigService::with_vars("fixture", []);
/// assert!(FixtureConfig::from_env(&env, pinned).is_err(), "a pin is held to the same range");
/// # Ok::<(), nest_rs_config::ConfigError>(())
/// ```
#[derive(Clone, Copy, Debug)]
pub struct DurationBounds {
    /// The unit the variable is written in, which its key's suffix spells.
    unit: DurationUnit,
    /// The floor.
    least: Floor,
    /// The ceiling.
    most: Bound,
}

impl DurationBounds {
    /// A range read in whole seconds, from a key ending in `_SECS`.
    pub const fn secs(least: Floor, most: Bound) -> Self {
        Self::new(DurationUnit::Seconds, least, most)
    }

    /// A range read in whole milliseconds, from a key ending in `_MS`.
    pub const fn millis(least: Floor, most: Bound) -> Self {
        Self::new(DurationUnit::Millis, least, most)
    }

    const fn new(unit: DurationUnit, least: Floor, most: Bound) -> Self {
        assert!(
            least.count() <= most.count,
            "a range's floor is at most its ceiling, or no value is inside it",
        );
        Self { unit, least, most }
    }

    /// The unit the variable is written in.
    pub const fn unit(&self) -> DurationUnit {
        self.unit
    }

    /// The floor.
    pub const fn least(&self) -> Floor {
        self.least
    }

    /// The ceiling, in the variable's unit.
    pub const fn most(&self) -> Bound {
        self.most
    }
}

/// The range one count setting accepts — how many of a thing the deployment
/// allows — each end with its reason. Its variable's key is its field's name.
///
/// A floor above the ceiling does not compile in a `const`.
///
/// ```
/// use nest_rs_config::{Bound, Config, ConfigService, CountBounds, config};
///
/// const WORKERS: CountBounds = CountBounds::new(
///     Bound::new(1, "no worker drains nothing"),
///     Bound::new(256, "past it the pool holds more connections than the database allows"),
/// );
///
/// #[config(namespace = "fixture")]
/// #[derive(Clone, Debug)]
/// pub struct FixtureConfig {
///     #[config(bounds = WORKERS)]
///     pub workers: usize,
/// }
/// # impl Default for FixtureConfig {
/// #     fn default() -> Self {
/// #         Self { workers: 4 }
/// #     }
/// # }
///
/// let env = ConfigService::with_vars("fixture", [("WORKERS", "300")]);
/// let refused = FixtureConfig::from_env(&env, FixtureConfig::default()).unwrap_err();
/// assert!(refused.to_string().contains("must be at most 256 — past it the pool"));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct CountBounds {
    least: Bound,
    most: Bound,
}

impl CountBounds {
    /// A count from `least` to `most`, each end with why nothing past it holds.
    pub const fn new(least: Bound, most: Bound) -> Self {
        assert!(
            least.count <= most.count,
            "a range's floor is at most its ceiling, or no value is inside it",
        );
        Self { least, most }
    }

    /// The floor.
    pub const fn least(&self) -> Bound {
        self.least
    }

    /// The ceiling.
    pub const fn most(&self) -> Bound {
        self.most
    }
}

/// A range bound to the field it holds: the key the field's variable is read
/// under — its name in SCREAMING_SNAKE, a duration's unit after it — and the
/// path code pins it through, `Config::field`.
///
/// `#[config(bounds = RANGE)]` binds one per field, as the associated constant
/// named for the field (`HttpConfig::REQUEST_TIMEOUT` for `request_timeout`),
/// and the derived overlay reads the field through it; every other site naming
/// the variable or holding a value built in code reaches the same constant, so
/// no site types the key. A hand-written overlay binds its own with
/// [`new`](Self::new).
pub struct BoundedSetting<T: BoundedField> {
    bounds: T::Bounds,
    field: Field,
}

impl<T: BoundedField> Clone for BoundedSetting<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: BoundedField> Copy for BoundedSetting<T> {}

impl<T: BoundedField> fmt::Debug for BoundedSetting<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundedSetting")
            .field("bounds", &self.bounds)
            .field("field", &self.field)
            .finish()
    }
}

impl<T: BoundedField> BoundedSetting<T> {
    /// `bounds`, holding `field` — what `#[config(bounds = …)]` writes, and
    /// what an overlay written by hand writes for itself.
    pub const fn new(bounds: T::Bounds, field: Field) -> Self {
        Self { bounds, field }
    }

    /// The same setting, its value pinned through `path` — a type built in code
    /// beside the config (`JwtOptions::leeway` beside `AuthnConfig::leeway`),
    /// whose refusal names the field its caller set, under the same variable.
    #[must_use]
    pub const fn pinned_through(self, path: &'static str) -> Self {
        Self {
            bounds: self.bounds,
            field: Field::new(self.field.key(), path),
        }
    }

    /// The range.
    pub const fn bounds(&self) -> T::Bounds {
        self.bounds
    }

    /// The field: the key its name spells, and the path code pins it through.
    pub const fn field(&self) -> Field {
        self.field
    }

    /// The variable's key in the namespace — `REQUEST_TIMEOUT_SECS` for a
    /// duration in seconds, the field's key itself for a count.
    pub fn key(&self) -> Cow<'static, str> {
        T::key(&self.bounds, self.field.key())
    }

    /// The variable, fully qualified under `namespace` —
    /// `<PREFIX>_<NAMESPACE>__<KEY>`.
    pub fn var(&self, namespace: &str) -> String {
        var_name(namespace, &self.key())
    }

    /// Every way the setting is given, as a sentence about it names them: the
    /// variable under `namespace`, then the field's path —
    /// `` <PREFIX>_<NAMESPACE>__<KEY>, or `Config::field` in code ``.
    pub fn var_or_field(&self, namespace: &str) -> String {
        var_name_or_field(namespace, &self.key(), self.field.path())
    }

    /// The setting's value — the variable's when it is set, `base` when it is
    /// not — refused outside the range, never clamped in silence.
    ///
    /// A value from the environment is refused under the spelling that supplied
    /// it, so one given as a file is refused under `_FILE`; `base` — pinned in
    /// code or the default — is refused under the variable that would override
    /// it, naming the field's path. A value that does not parse as a whole
    /// number is refused as [`Setting::parse`](crate::Setting::parse) refuses
    /// it. For an `Option<Duration>`, `0` from the environment is `None`, and
    /// `None` in code stays `None`.
    pub fn read(&self, env: &ConfigService, base: T) -> Result<T, ConfigError> {
        T::read(self, env, base)
    }

    /// `value`, built in code with no reader in hand, held to the same range
    /// under the variable `namespace` reads it from — for a constructor a
    /// config and a hand-built value both reach, which must refuse what the
    /// variable would have been refused for.
    pub fn check(&self, namespace: &str, value: T) -> Result<T, ConfigError> {
        T::check(self, namespace, value)
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for std::time::Duration {}
    impl Sealed for Option<std::time::Duration> {}
    impl Sealed for usize {}
}

/// A field type a range holds, and the range that holds it: a `Duration` and
/// an `Option<Duration>` — `None` off, `0` from the environment — through
/// [`DurationBounds`], a `usize` through [`CountBounds`]. Sealed.
#[diagnostic::on_unimplemented(
    message = "`#[config(bounds = …)]` reads a `Duration`, an `Option<Duration>` or a `usize`, \
               not `{Self}`",
    label = "a field its bounds read",
    note = "a duration is a `Duration`, or an `Option<Duration>` where `None` turns it off; a \
            count is a `usize`, and one whose `None` is unlimited reads `#[config(count)]`"
)]
pub trait BoundedField: Sized + sealed::Sealed {
    /// The range a field of this type is held to.
    type Bounds: Copy + fmt::Debug;

    /// `BoundedSetting<Self>`, which `#[config(bounds = …)]` names through this
    /// trait, so a type no range holds is refused in one sentence rather than
    /// once per item the derive writes.
    type Setting;

    #[doc(hidden)]
    fn key(bounds: &Self::Bounds, stem: &'static str) -> Cow<'static, str>;

    #[doc(hidden)]
    fn read(
        setting: &BoundedSetting<Self>,
        env: &ConfigService,
        base: Self,
    ) -> Result<Self, ConfigError>;

    #[doc(hidden)]
    fn check(
        setting: &BoundedSetting<Self>,
        namespace: &str,
        value: Self,
    ) -> Result<Self, ConfigError>;
}

impl BoundedField for Duration {
    type Bounds = DurationBounds;
    type Setting = BoundedSetting<Self>;

    fn key(bounds: &DurationBounds, stem: &'static str) -> Cow<'static, str> {
        Cow::Owned(format!("{stem}{}", bounds.unit.suffix()))
    }

    fn read(
        setting: &BoundedSetting<Self>,
        env: &ConfigService,
        base: Self,
    ) -> Result<Self, ConfigError> {
        let key = setting.key();
        match read_duration(&setting.bounds, env, &key, Off::Absent)? {
            Some(Some(read)) => Ok(read),
            // Asked with no off to give, the read never answers it.
            Some(None) | None => pinned_duration(
                setting,
                env.var_name(&key),
                base,
                "is not set, and ",
                Off::Absent,
            ),
        }
    }

    fn check(
        setting: &BoundedSetting<Self>,
        namespace: &str,
        value: Self,
    ) -> Result<Self, ConfigError> {
        pinned_duration(setting, setting.var(namespace), value, "", Off::Absent)
    }
}

impl BoundedField for Option<Duration> {
    type Bounds = DurationBounds;
    type Setting = BoundedSetting<Self>;

    fn key(bounds: &DurationBounds, stem: &'static str) -> Cow<'static, str> {
        Duration::key(bounds, stem)
    }

    fn read(
        setting: &BoundedSetting<Self>,
        env: &ConfigService,
        base: Self,
    ) -> Result<Self, ConfigError> {
        let key = setting.key();
        match read_duration(&setting.bounds, env, &key, Off::AtZero)? {
            Some(read) => Ok(read),
            None => base
                .map(|base| {
                    pinned_duration(
                        setting,
                        env.var_name(&key),
                        base,
                        "is not set, and ",
                        Off::AtZero,
                    )
                })
                .transpose(),
        }
    }

    fn check(
        setting: &BoundedSetting<Self>,
        namespace: &str,
        value: Self,
    ) -> Result<Self, ConfigError> {
        value
            .map(|value| pinned_duration(setting, setting.var(namespace), value, "", Off::AtZero))
            .transpose()
    }
}

impl BoundedField for usize {
    type Bounds = CountBounds;
    type Setting = BoundedSetting<Self>;

    fn key(_: &CountBounds, stem: &'static str) -> Cow<'static, str> {
        Cow::Borrowed(stem)
    }

    fn read(
        setting: &BoundedSetting<Self>,
        env: &ConfigService,
        base: Self,
    ) -> Result<Self, ConfigError> {
        let Some(read) = env.setting(setting.field.key())? else {
            return pinned_count(
                setting,
                env.var_name(setting.field.key()),
                base,
                "is not set, and ",
            );
        };
        let count: Self = read.parse()?;
        match outside(&setting.bounds, count) {
            None => Ok(count),
            Some(Past { must, bound, .. }) => Err(read.refuse(format_args!(
                "must be {must} {} — {}",
                bound.count, bound.why
            ))),
        }
    }

    fn check(
        setting: &BoundedSetting<Self>,
        namespace: &str,
        value: Self,
    ) -> Result<Self, ConfigError> {
        pinned_count(setting, setting.var(namespace), value, "")
    }
}

/// Whether the field a duration is read into has an off: `None`, `0` from the
/// environment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Off {
    /// A `Duration`: `0` is held to the floor.
    Absent,
    /// An `Option<Duration>`: `0` is `None`.
    AtZero,
}

/// The variable under `key`, held to `bounds`: `None` when it is unset, and
/// `Some(None)` for `0` where the field has an off.
fn read_duration(
    bounds: &DurationBounds,
    env: &ConfigService,
    key: &str,
    off: Off,
) -> Result<Option<Option<Duration>>, ConfigError> {
    let Some(setting) = env.duration_setting(key)? else {
        return Ok(None);
    };
    let count = setting.parse::<u64>()?;
    if off == Off::AtZero && count == 0 {
        return Ok(Some(None));
    }
    let or_off = match off {
        Off::AtZero => ", or 0 to turn it off",
        Off::Absent => "",
    };
    let (least, why) = match bounds.least {
        Floor::Units(bound) => (bound.count, bound.why),
        Floor::AboveZero(why) => (1, why),
    };
    if count < least {
        return Err(setting.refuse(format_args!(
            "must be at least {}{or_off} — {why}",
            Count(bounds.unit, least),
        )));
    }
    if count > bounds.most.count {
        return Err(setting.refuse(format_args!(
            "must be at most {}{or_off} — {}",
            Count(bounds.unit, bounds.most.count),
            bounds.most.why,
        )));
    }
    Ok(Some(Some(bounds.unit.duration(count))))
}

/// `value`, set in code through `setting`'s field, refused under `var` when it
/// is outside the range. `lead` opens the sentence with what is known of the
/// variable.
fn pinned_duration<T: BoundedField<Bounds = DurationBounds>>(
    setting: &BoundedSetting<T>,
    var: String,
    value: Duration,
    lead: &str,
    off: Off,
) -> Result<Duration, ConfigError> {
    let bounds = setting.bounds;
    let path = setting.field.path();
    let or_off = match off {
        Off::AtZero => "; `None` turns it off",
        Off::Absent => "",
    };
    let refuse = |side: &str, limit: Duration, must: &str, why: &str| {
        ConfigError::parse(
            var.clone(),
            format!(
                "{lead}`{path}` set in code is {value:?}, {side} the {limit:?} it must be {must} — \
                 {why}{or_off}",
            ),
        )
    };
    match bounds.least {
        Floor::Units(bound) => {
            let least = bounds.unit.duration(bound.count);
            if value < least {
                return Err(refuse("below", least, "at least", bound.why));
            }
        }
        Floor::AboveZero(why) if value.is_zero() => {
            return Err(ConfigError::parse(
                var.clone(),
                format!(
                    "{lead}`{path}` set in code is {value:?}, and it must be above zero — \
                     {why}{or_off}",
                ),
            ));
        }
        Floor::AboveZero(_) => {}
    }
    let most = bounds.unit.duration(bounds.most.count);
    if value > most {
        return Err(refuse("above", most, "at most", bounds.most.why));
    }
    Ok(value)
}

/// `count`, set in code through `setting`'s field, refused under `var` when it
/// is outside the range; `lead` opens the sentence with what is known of the
/// variable.
fn pinned_count(
    setting: &BoundedSetting<usize>,
    var: String,
    count: usize,
    lead: &str,
) -> Result<usize, ConfigError> {
    match outside(&setting.bounds, count) {
        None => Ok(count),
        Some(Past { side, must, bound }) => Err(ConfigError::parse(
            var,
            format!(
                "{lead}`{}` set in code is {count}, {side} the {} it must be {must} — {}",
                setting.field.path(),
                bound.count,
                bound.why,
            ),
        )),
    }
}

/// The end of a count's range a value falls past.
struct Past {
    side: &'static str,
    must: &'static str,
    bound: Bound,
}

/// Which end of `bounds` `count` falls past, if either.
fn outside(bounds: &CountBounds, count: usize) -> Option<Past> {
    let count = u64::try_from(count).unwrap_or(u64::MAX);
    if count < bounds.least.count {
        Some(Past {
            side: "below",
            must: "at least",
            bound: bounds.least,
        })
    } else if count > bounds.most.count {
        Some(Past {
            side: "above",
            must: "at most",
            bound: bounds.most,
        })
    } else {
        None
    }
}

/// `count` followed by its unit, singular or plural.
struct Count(DurationUnit, u64);

impl fmt::Display for Count {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.1, self.0.name(self.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGE: DurationBounds = DurationBounds::secs(
        Floor::Units(Bound::new(1, "the floor's reason")),
        Bound::new(60, "the ceiling's reason"),
    );

    const WINDOW: BoundedSetting<Duration> =
        BoundedSetting::new(RANGE, Field::new("WINDOW", "FixtureConfig::window"));

    const CEILING: BoundedSetting<Option<Duration>> = BoundedSetting::new(
        DurationBounds::secs(
            Floor::Units(Bound::new(10, "the floor's reason")),
            Bound::new(60, "the ceiling's reason"),
        ),
        Field::new("CEILING", "FixtureConfig::ceiling"),
    );

    fn env(vars: &[(&str, &str)]) -> ConfigService {
        ConfigService::with_vars("fixture", vars.iter().copied())
    }

    fn refused<T: fmt::Debug>(result: Result<T, ConfigError>) -> String {
        result.expect_err("refused").to_string()
    }

    #[test]
    fn the_key_is_the_fields_name_with_the_units_suffix_after_it() {
        assert_eq!(WINDOW.key(), "WINDOW_SECS");
        let millis: BoundedSetting<Duration> = BoundedSetting::new(
            DurationBounds::millis(Floor::AboveZero("why"), Bound::new(5, "why")),
            Field::new("DEADLINE", "FixtureConfig::deadline"),
        );
        assert_eq!(millis.key(), "DEADLINE_MS");
        assert_eq!(WINDOW.var("fixture"), var_name("fixture", "WINDOW_SECS"));
    }

    #[test]
    fn a_value_inside_the_range_is_read_from_either_side() {
        let read = WINDOW
            .read(&env(&[("WINDOW_SECS", "60")]), Duration::from_secs(5))
            .expect("the ceiling itself is accepted");
        assert_eq!(read, Duration::from_secs(60));

        let pinned = WINDOW
            .read(&env(&[]), Duration::from_millis(1500))
            .expect("a pin inside the range, sub-unit included");
        assert_eq!(pinned, Duration::from_millis(1500));
    }

    #[test]
    fn the_environment_is_refused_past_either_end_naming_the_variable_and_the_unit() {
        let below = refused(WINDOW.read(&env(&[("WINDOW_SECS", "0")]), Duration::from_secs(5)));
        assert!(
            below.contains(&var_name("fixture", "WINDOW_SECS"))
                && below.contains("must be at least 1 second — the floor's reason"),
            "{below}",
        );
        let above = refused(WINDOW.read(&env(&[("WINDOW_SECS", "61")]), Duration::from_secs(5)));
        assert!(
            above.contains("must be at most 60 seconds — the ceiling's reason"),
            "{above}"
        );
    }

    #[test]
    fn a_pin_is_held_to_the_range_the_variable_is() {
        let below = refused(WINDOW.read(&env(&[]), Duration::ZERO));
        assert!(
            below.contains(&var_name("fixture", "WINDOW_SECS"))
                && below.contains("is not set, and `FixtureConfig::window` set in code is 0ns")
                && below.contains("below the 1s it must be at least — the floor's reason"),
            "{below}",
        );
        let above = refused(WINDOW.read(&env(&[]), Duration::from_secs(61)));
        assert!(
            above.contains("above the 60s it must be at most — the ceiling's reason"),
            "{above}"
        );
    }

    #[test]
    fn a_value_built_in_code_is_refused_naming_the_field_it_was_built_through() {
        let text = refused(WINDOW.check("fixture", Duration::ZERO));
        assert!(
            text.contains("`FixtureConfig::window` set in code is 0ns")
                && !text.contains("is not set"),
            "{text}",
        );
        assert_eq!(
            WINDOW
                .check("fixture", Duration::from_secs(3))
                .expect("inside"),
            Duration::from_secs(3),
        );
    }

    #[test]
    fn a_millisecond_unit_names_itself() {
        const PROBE: BoundedSetting<Duration> = BoundedSetting::new(
            DurationBounds::millis(
                Floor::Units(Bound::new(1, "why")),
                Bound::new(60_000, "past a minute"),
            ),
            Field::new("DEADLINE", "FixtureConfig::deadline"),
        );
        let text = refused(PROBE.read(&env(&[("DEADLINE_MS", "0")]), Duration::from_millis(5)));
        assert!(text.contains("must be at least 1 millisecond"), "{text}");
        let text = refused(PROBE.read(
            &env(&[("DEADLINE_MS", "18446744073709551615")]),
            Duration::from_millis(5),
        ));
        assert!(
            text.contains("must be at most 60000 milliseconds — past a minute"),
            "{text}"
        );
    }

    /// The top of `u64` is past every ceiling, and is refused naming its variable.
    #[test]
    fn the_top_of_the_range_is_refused_from_either_side() {
        let text = refused(WINDOW.read(
            &env(&[("WINDOW_SECS", "18446744073709551615")]),
            Duration::from_secs(5),
        ));
        assert!(
            text.contains(&var_name("fixture", "WINDOW_SECS"))
                && text.contains("must be at most 60 seconds"),
            "{text}"
        );
        let text = refused(WINDOW.read(&env(&[]), Duration::MAX));
        assert!(text.contains("above the 60s it must be at most"), "{text}");
    }

    /// An `Option<Duration>` reads `0` as `None`, holds every other value to
    /// the range, and refuses a pinned zero — off in code is `None`, never a
    /// zero that would cut every connection. Unset, its `None` stays `None`.
    #[test]
    fn an_off_switch_is_zero_from_the_environment_and_none_in_code() {
        assert!(
            CEILING
                .read(
                    &env(&[("CEILING_SECS", "0")]),
                    Some(Duration::from_secs(30))
                )
                .expect("off")
                .is_none(),
            "`0` is off, over a pinned value too",
        );
        assert!(CEILING.read(&env(&[]), None).expect("off").is_none());
        let below = refused(CEILING.read(&env(&[("CEILING_SECS", "3")]), None));
        assert!(
            below.contains("must be at least 10 seconds, or 0 to turn it off — the floor's reason"),
            "{below}"
        );
        let above = refused(CEILING.read(&env(&[("CEILING_SECS", "61")]), None));
        assert!(
            above
                .contains("must be at most 60 seconds, or 0 to turn it off — the ceiling's reason"),
            "{above}"
        );
        let pinned = refused(CEILING.read(&env(&[]), Some(Duration::ZERO)));
        assert!(
            pinned.contains("set in code is 0ns, below the 10s it must be at least")
                && pinned.contains("`None` turns it off"),
            "{pinned}"
        );
        let held = refused(CEILING.check("fixture", Some(Duration::ZERO)));
        assert!(
            held.contains("`None` turns it off") && !held.contains("is not set"),
            "{held}"
        );
        assert_eq!(CEILING.check("fixture", None).expect("off"), None);
    }

    /// A floor whose only failure is zero holds the variable to one unit, and a
    /// value set in code to anything above zero.
    #[test]
    fn a_floor_above_zero_takes_a_sub_unit_value_set_in_code() {
        const BUDGET: BoundedSetting<Duration> = BoundedSetting::new(
            DurationBounds::secs(
                Floor::AboveZero("a zero budget gives up before its first attempt"),
                Bound::new(3600, "why"),
            ),
            Field::new("BUDGET", "FixtureConfig::budget"),
        );
        let from_env = refused(BUDGET.read(&env(&[("BUDGET_SECS", "0")]), Duration::from_secs(5)));
        assert!(
            from_env.contains("must be at least 1 second — a zero budget"),
            "{from_env}"
        );
        let pinned = refused(BUDGET.read(&env(&[]), Duration::ZERO));
        assert!(
            pinned.contains("set in code is 0ns, and it must be above zero — a zero budget"),
            "{pinned}"
        );
        let sub_unit = BUDGET
            .read(&env(&[]), Duration::from_millis(300))
            .expect("a sub-unit budget set in code is a budget");
        assert_eq!(sub_unit, Duration::from_millis(300));
    }

    #[test]
    fn a_value_from_a_file_is_refused_under_its_file_spelling() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("window");
        std::fs::write(&path, "0\n").expect("write");
        let path = path.to_string_lossy().into_owned();
        let text =
            refused(WINDOW.read(&env(&[("WINDOW_SECS_FILE", &path)]), Duration::from_secs(5)));
        assert!(
            text.contains(&var_name("fixture", "WINDOW_SECS_FILE")),
            "{text}"
        );
    }

    const SESSIONS: BoundedSetting<usize> = BoundedSetting::new(
        CountBounds::new(
            Bound::new(1, "the floor's reason"),
            Bound::new(1024, "the ceiling's reason"),
        ),
        Field::new("SESSIONS", "FixtureConfig::sessions"),
    );

    #[test]
    fn a_count_reads_its_key_and_is_held_to_its_range_from_either_side() {
        assert_eq!(SESSIONS.key(), "SESSIONS");
        assert_eq!(
            SESSIONS
                .read(&env(&[("SESSIONS", "1024")]), 3)
                .expect("the ceiling itself"),
            1024
        );
        assert_eq!(
            SESSIONS.read(&env(&[]), 3).expect("unset keeps the base"),
            3
        );
        let below = refused(SESSIONS.read(&env(&[("SESSIONS", "0")]), 3));
        assert!(
            below.contains(&var_name("fixture", "SESSIONS"))
                && below.contains("must be at least 1 — the floor's reason"),
            "{below}"
        );
        let above = refused(SESSIONS.read(&env(&[("SESSIONS", "1025")]), 3));
        assert!(
            above.contains("must be at most 1024 — the ceiling's reason"),
            "{above}"
        );
        let unparsed = refused(SESSIONS.read(&env(&[("SESSIONS", "-1")]), 3));
        assert!(!unparsed.contains("-1"), "{unparsed}");
        let pinned = refused(SESSIONS.read(&env(&[]), usize::MAX));
        assert!(
            pinned.contains(&format!(
                "is not set, and `FixtureConfig::sessions` set in code is {}, above the 1024 it \
                 must be at most — the ceiling's reason",
                usize::MAX
            )),
            "{pinned}"
        );
        let held = refused(SESSIONS.check("fixture", 0));
        assert!(
            held.contains("`FixtureConfig::sessions` set in code is 0, below the 1")
                && !held.contains("is not set"),
            "{held}"
        );
    }
}
