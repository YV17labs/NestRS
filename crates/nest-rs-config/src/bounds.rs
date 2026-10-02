//! [`DurationBounds`] — the one reader for a duration a deployment sets, and the
//! one sentence refusing a value outside its range.
//!
//! **Why it is here rather than beside any one duration.** Three crates wrote
//! their own — a range struct in `nest-rs-http`, another in the Redis worker, an
//! inline zero check in the Redis connection — each with its own wording and its
//! own idea of whether a value pinned in code was held to the range. The inline
//! one was not, and a pinned zero budget then booted into "could not reach Redis
//! … within 0ns (0 attempt(s))" against a Redis that answered. A range checked on
//! one path and not the other is the drift one reader removes: the environment
//! and the pin are refused by the same code, in the same words.
//!
//! **Every range has both ends.** The floor is where the thing stops working; the
//! ceiling is where a value stops being a setting and becomes a slip — and it is
//! never above what the library or the kernel the value reaches accepts, so no
//! value the boot accepts can panic or be refused below it. A Redis budget past
//! the kernel's keepalive limit failed every dial with `EINVAL`, and a SeaORM
//! budget past what an `Instant` holds panicked inside sqlx: both were values
//! with a floor and no ceiling. [`most`](DurationBounds::most) is therefore not
//! optional.
//!
//! **`0` is an off switch only where the declaration says so**
//! ([`Floor::UnitsOrOff`]): a connection ceiling lifted, a keep-alive not sent —
//! the settings whose absence a deployment may choose. Everywhere else *off* is
//! the defect the floor bounds — an unbounded connect, a probe that outlives the
//! kubelet, a rate limit that limits nothing — and `0` is refused.

use std::fmt;
use std::time::Duration;

use crate::error::ConfigError;
use crate::service::{ConfigService, var_name};

/// The whole-number unit a duration's variable is written in — the suffix its
/// key carries, `_SECS` or `_MS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    fn name(self, count: u64) -> &'static str {
        match (self, count) {
            (Self::Seconds, 1) => "second",
            (Self::Seconds, _) => "seconds",
            (Self::Millis, 1) => "millisecond",
            (Self::Millis, _) => "milliseconds",
        }
    }
}

/// One end of a range, in its variable's unit, and why nothing past it holds —
/// the reason a refusal gives, so it is written for the operator reading the
/// boot error.
#[derive(Clone, Copy, Debug)]
pub struct Bound {
    /// The end, in the variable's [`DurationUnit`].
    pub count: u64,
    /// Why a value past this end cannot work — never quoting the value.
    pub why: &'static str,
}

/// Where a range starts.
#[derive(Clone, Copy, Debug)]
pub enum Floor {
    /// At least `count` of the variable's unit, whichever side set the value —
    /// for a floor that is a property of the thing bounded: a lease needs a
    /// second to renew in, a shutdown window under one cuts every request.
    Units(Bound),
    /// Longer than nothing — for a bound whose only failure is zero, a budget
    /// that gives up before its first attempt. The variable, written in whole
    /// units, is held to one of them; a value set in code to anything above
    /// zero, a sub-unit budget included, is taken. The reason is why zero fails.
    AboveZero(&'static str),
    /// At least `count` of the variable's unit — or **off**: `0` from the
    /// environment, `None` in code. For a bound whose absence is a choice a
    /// deployment may make deliberately — a connection ceiling lifted, a
    /// keep-alive not sent — where reading `0` as *zero* would turn a ceiling
    /// into a kill switch. Off has a value only through
    /// [`read_optional`](DurationBounds::read_optional);
    /// [`read`](DurationBounds::read) has none to return and holds `0` to the
    /// floor.
    UnitsOrOff(Bound),
}

/// The range one duration setting accepts, the variable that sets it, and the
/// field that pins it in code.
///
/// Declared as a `const` beside the config that reads it, so the numbers, the
/// reasons and the field doc stay one screen apart:
///
/// ```
/// use std::time::Duration;
/// use nest_rs_config::{Bound, ConfigService, DurationBounds, DurationUnit, Floor};
///
/// const WINDOW: DurationBounds = DurationBounds {
///     key: "WINDOW_SECS",
///     field: "FixtureConfig::window",
///     unit: DurationUnit::Seconds,
///     least: Floor::Units(Bound { count: 1, why: "a shorter window counts nothing" }),
///     most: Bound { count: 3600, why: "past an hour it is a unit slip" },
/// };
///
/// let env = ConfigService::with_vars("fixture", [("WINDOW_SECS", "30")]);
/// assert_eq!(WINDOW.read(&env, Duration::from_secs(60))?.value, Duration::from_secs(30));
///
/// let zero = ConfigService::with_vars("fixture", [("WINDOW_SECS", "0")]);
/// assert!(WINDOW.read(&zero, Duration::from_secs(60)).is_err());
///
/// let pinned = ConfigService::with_vars("fixture", []);
/// assert!(WINDOW.read(&pinned, Duration::ZERO).is_err(), "a pin is held to the same range");
///
/// // A ceiling a deployment may lift says so in its floor, and `0` reads as off.
/// const CEILING: DurationBounds = DurationBounds {
///     least: Floor::UnitsOrOff(Bound { count: 1, why: "a shorter ceiling ends every stream" }),
///     ..WINDOW
/// };
/// let off = ConfigService::with_vars("fixture", [("WINDOW_SECS", "0")]);
/// assert!(CEILING.read_optional(&off, Some(Duration::from_secs(60)))?.is_none());
/// # Ok::<(), nest_rs_config::ConfigError>(())
/// ```
#[derive(Clone, Copy, Debug)]
pub struct DurationBounds {
    /// The variable's key in the namespace — `SHUTDOWN_TIMEOUT_SECS`.
    pub key: &'static str,
    /// The field the value is pinned through in code, as a caller writes it —
    /// `HttpConfig::shutdown_timeout`.
    pub field: &'static str,
    /// The unit the variable is written in.
    pub unit: DurationUnit,
    /// The floor.
    pub least: Floor,
    /// The ceiling — where a value stops being a setting and becomes a slip,
    /// never above what the library or the kernel the value reaches accepts.
    /// A setting whose range ends at another setting's value states here the
    /// end that setting's own ceiling implies, and checks the nearer end once
    /// both are read, through [`BoundedDuration::refuse`].
    pub most: Bound,
}

/// What the environment says about a setting.
enum Variable {
    /// Neither spelling is set.
    Unset,
    /// `0`, under a [`Floor::UnitsOrOff`] read through
    /// [`read_optional`](DurationBounds::read_optional).
    Off,
    /// A value inside the range.
    Set(BoundedDuration),
}

impl DurationBounds {
    /// The setting's value — the variable's when it is set, `base` when it is
    /// not — refused outside the range, never clamped in silence.
    ///
    /// A value from the environment is refused under the spelling that supplied
    /// it, so one given as a file is refused under `_FILE`; `base` — pinned in
    /// code or the default — is refused under the variable that would override
    /// it, naming [`field`](Self::field). A value that does not parse as a whole
    /// number is refused as [`Setting::parse`](crate::Setting::parse) refuses it.
    pub fn read(
        &self,
        env: &ConfigService,
        base: Duration,
    ) -> Result<BoundedDuration, ConfigError> {
        // Asked without a `None` to give it, `variable` never answers `Off`: a
        // `0` under `Floor::UnitsOrOff` is held to the floor instead.
        match self.variable(env, false)? {
            Variable::Set(read) => Ok(read),
            Variable::Unset | Variable::Off => {
                self.pinned(env.var_name(self.key), base, "is not set, and ")
            }
        }
    }

    /// [`read`](Self::read) for a setting whose absence means something — the
    /// library's own default, or off under [`Floor::UnitsOrOff`]: `None` stays
    /// `None` when the variable is unset too, `0` is `None` where the floor
    /// admits off, and a value from either side is held to the range.
    pub fn read_optional(
        &self,
        env: &ConfigService,
        base: Option<Duration>,
    ) -> Result<Option<BoundedDuration>, ConfigError> {
        match self.variable(env, true)? {
            Variable::Set(read) => Ok(Some(read)),
            Variable::Off => Ok(None),
            Variable::Unset => base
                .map(|base| self.pinned(env.var_name(self.key), base, "is not set, and "))
                .transpose(),
        }
    }

    /// `value`, built in code with no reader in hand, held to the same range —
    /// for a constructor a config and a hand-built value both reach, which must
    /// refuse what the variable would have been refused for. `field` names what
    /// the caller set, since that is not always [`field`](Self::field).
    pub fn check(
        &self,
        namespace: &str,
        field: &'static str,
        value: Duration,
    ) -> Result<Duration, ConfigError> {
        let pinned = Self { field, ..*self };
        pinned
            .pinned(var_name(namespace, self.key), value, "")
            .map(|read| read.value)
    }

    /// The variable, read and held to the range; `0` is [`Variable::Off`] when
    /// the floor admits off and the caller has a `None` to give it.
    fn variable(&self, env: &ConfigService, may_be_off: bool) -> Result<Variable, ConfigError> {
        let Some(setting) = env.setting(self.key)? else {
            return Ok(Variable::Unset);
        };
        let count = setting.parse::<u64>()?;
        let (least, why, off) = match self.least {
            Floor::Units(bound) => (bound.count, bound.why, false),
            Floor::AboveZero(why) => (1, why, false),
            Floor::UnitsOrOff(bound) => (bound.count, bound.why, may_be_off),
        };
        if off && count == 0 {
            return Ok(Variable::Off);
        }
        let or_off = if off { ", or 0 to turn it off" } else { "" };
        if count < least {
            return Err(setting.refuse(format_args!(
                "must be at least {}{or_off} — {why}",
                Count(self.unit, least),
            )));
        }
        if count > self.most.count {
            return Err(setting.refuse(format_args!(
                "must be at most {}{or_off} — {}",
                Count(self.unit, self.most.count),
                self.most.why,
            )));
        }
        Ok(Variable::Set(BoundedDuration {
            value: self.unit.duration(count),
            var: setting.var().to_owned(),
        }))
    }

    /// `value`, set in code, refused under `var` when it is outside the range.
    /// `lead` opens the sentence with what is known of the variable.
    fn pinned(
        &self,
        var: String,
        value: Duration,
        lead: &str,
    ) -> Result<BoundedDuration, ConfigError> {
        let or_off = match self.least {
            Floor::UnitsOrOff(_) => "; `None` turns it off",
            Floor::Units(_) | Floor::AboveZero(_) => "",
        };
        let refuse = |side: &str, limit: Duration, must: &str, why: &str| {
            ConfigError::parse(
                var.clone(),
                format!(
                    "{lead}`{}` set in code is {value:?}, {side} the {limit:?} it must be {must} — \
                     {why}{or_off}",
                    self.field,
                ),
            )
        };
        match self.least {
            Floor::Units(bound) | Floor::UnitsOrOff(bound) => {
                let least = self.unit.duration(bound.count);
                if value < least {
                    return Err(refuse("below", least, "at least", bound.why));
                }
            }
            Floor::AboveZero(why) if value.is_zero() => {
                return Err(ConfigError::parse(
                    var.clone(),
                    format!(
                        "{lead}`{}` set in code is {value:?}, and it must be above zero — {why}",
                        self.field,
                    ),
                ));
            }
            Floor::AboveZero(_) => {}
        }
        let most = self.unit.duration(self.most.count);
        if value > most {
            return Err(refuse("above", most, "at most", self.most.why));
        }
        Ok(BoundedDuration { value, var })
    }
}

/// A duration setting as read, and the variable a refusal names it by: the
/// spelling that supplied it, or the one that would override the value set in
/// code.
#[derive(Clone, Debug)]
pub struct BoundedDuration {
    /// The value, inside its range.
    pub value: Duration,
    var: String,
}

impl BoundedDuration {
    /// The variable a refusal of this value names.
    pub fn var(&self) -> &str {
        &self.var
    }

    /// The boot error refusing this value for `reason` — a check against
    /// another setting's value, made once both are read. `reason` must not quote
    /// a value read from a file.
    pub fn refuse(&self, reason: impl fmt::Display) -> ConfigError {
        ConfigError::parse(self.var.clone(), reason.to_string())
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

    const RANGE: DurationBounds = DurationBounds {
        key: "WINDOW_SECS",
        field: "FixtureConfig::window",
        unit: DurationUnit::Seconds,
        least: Floor::Units(Bound {
            count: 1,
            why: "the floor's reason",
        }),
        most: Bound {
            count: 60,
            why: "the ceiling's reason",
        },
    };

    fn env(vars: &[(&str, &str)]) -> ConfigService {
        ConfigService::with_vars("fixture", vars.iter().copied())
    }

    fn refused(result: Result<BoundedDuration, ConfigError>) -> String {
        result.expect_err("refused").to_string()
    }

    #[test]
    fn a_value_inside_the_range_is_read_from_either_side() {
        let read = RANGE
            .read(&env(&[("WINDOW_SECS", "60")]), Duration::from_secs(5))
            .expect("the ceiling itself is accepted");
        assert_eq!(read.value, Duration::from_secs(60));
        assert_eq!(read.var(), var_name("fixture", "WINDOW_SECS"));

        let pinned = RANGE
            .read(&env(&[]), Duration::from_millis(1500))
            .expect("a pin inside the range, sub-unit included");
        assert_eq!(pinned.value, Duration::from_millis(1500));
    }

    #[test]
    fn the_environment_is_refused_past_either_end_naming_the_variable_and_the_unit() {
        let below = refused(RANGE.read(&env(&[("WINDOW_SECS", "0")]), Duration::from_secs(5)));
        assert!(
            below.contains(&var_name("fixture", "WINDOW_SECS"))
                && below.contains("must be at least 1 second — the floor's reason"),
            "{below}",
        );
        let above = refused(RANGE.read(&env(&[("WINDOW_SECS", "61")]), Duration::from_secs(5)));
        assert!(
            above.contains("must be at most 60 seconds — the ceiling's reason"),
            "{above}"
        );
    }

    #[test]
    fn a_pin_is_held_to_the_range_the_variable_is() {
        let below = refused(RANGE.read(&env(&[]), Duration::ZERO));
        assert!(
            below.contains(&var_name("fixture", "WINDOW_SECS"))
                && below.contains("is not set, and `FixtureConfig::window` set in code is 0ns")
                && below.contains("below the 1s it must be at least — the floor's reason"),
            "{below}",
        );
        let above = refused(RANGE.read(&env(&[]), Duration::from_secs(61)));
        assert!(
            above.contains("above the 60s it must be at most — the ceiling's reason"),
            "{above}"
        );
    }

    #[test]
    fn an_optional_setting_stays_unset_and_is_held_to_the_range_once_set() {
        assert!(
            RANGE
                .read_optional(&env(&[]), None)
                .expect("unset")
                .is_none()
        );
        let pinned = RANGE
            .read_optional(&env(&[]), Some(Duration::ZERO))
            .expect_err("a pinned zero is refused");
        assert!(
            pinned.to_string().contains("set in code is 0ns"),
            "{pinned}"
        );
        let read = RANGE
            .read_optional(&env(&[("WINDOW_SECS", "7")]), None)
            .expect("read")
            .expect("set");
        assert_eq!(read.value, Duration::from_secs(7));
    }

    #[test]
    fn a_value_built_in_code_is_refused_naming_the_field_it_was_built_through() {
        let err = RANGE
            .check("fixture", "FixtureOptions::window", Duration::ZERO)
            .expect_err("refused");
        let text = err.to_string();
        assert!(
            text.contains("`FixtureOptions::window` set in code is 0ns")
                && !text.contains("is not set"),
            "{text}",
        );
        assert_eq!(
            RANGE
                .check("fixture", "FixtureOptions::window", Duration::from_secs(3))
                .expect("inside"),
            Duration::from_secs(3),
        );
    }

    #[test]
    fn a_millisecond_unit_names_itself() {
        const PROBE: DurationBounds = DurationBounds {
            key: "DEADLINE_MS",
            field: "FixtureConfig::deadline",
            unit: DurationUnit::Millis,
            least: Floor::Units(Bound {
                count: 1,
                why: "why",
            }),
            most: Bound {
                count: 60_000,
                why: "past a minute",
            },
        };
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

    /// Every range has a ceiling, and the top of `u64` is past every one of them:
    /// the value that panicked sqlx inside the boot is refused naming its variable.
    #[test]
    fn the_top_of_the_range_is_refused_from_either_side() {
        let text = refused(RANGE.read(
            &env(&[("WINDOW_SECS", "18446744073709551615")]),
            Duration::from_secs(5),
        ));
        assert!(
            text.contains(&var_name("fixture", "WINDOW_SECS"))
                && text.contains("must be at most 60 seconds"),
            "{text}"
        );
        let text = refused(RANGE.read(&env(&[]), Duration::MAX));
        assert!(text.contains("above the 60s it must be at most"), "{text}");
    }

    /// A floor that admits off reads `0` as `None` where the caller has one to
    /// give, holds every other value to the range, and refuses a pinned zero —
    /// off in code is `None`, never a zero that would cut every connection.
    #[test]
    fn an_off_switch_is_zero_from_the_environment_and_none_in_code() {
        const CEILING: DurationBounds = DurationBounds {
            least: Floor::UnitsOrOff(Bound {
                count: 10,
                why: "the floor's reason",
            }),
            ..RANGE
        };
        assert!(
            CEILING
                .read_optional(&env(&[("WINDOW_SECS", "0")]), Some(Duration::from_secs(30)))
                .expect("off")
                .is_none(),
            "`0` is off, over a pinned value too",
        );
        assert!(
            CEILING
                .read_optional(&env(&[]), None)
                .expect("off")
                .is_none()
        );
        let below = refused(
            CEILING
                .read_optional(&env(&[("WINDOW_SECS", "3")]), None)
                .map(|read| read.expect("set")),
        );
        assert!(
            below.contains("must be at least 10 seconds, or 0 to turn it off — the floor's reason"),
            "{below}"
        );
        let above = refused(
            CEILING
                .read_optional(&env(&[("WINDOW_SECS", "61")]), None)
                .map(|read| read.expect("set")),
        );
        assert!(
            above
                .contains("must be at most 60 seconds, or 0 to turn it off — the ceiling's reason"),
            "{above}"
        );
        let pinned = refused(
            CEILING
                .read_optional(&env(&[]), Some(Duration::ZERO))
                .map(|read| read.expect("set")),
        );
        assert!(
            pinned.contains("set in code is 0ns, below the 10s it must be at least")
                && pinned.contains("`None` turns it off"),
            "{pinned}"
        );
        let held = refused(CEILING.read(&env(&[("WINDOW_SECS", "0")]), Duration::from_secs(30)));
        assert!(
            held.contains("must be at least 10 seconds — the floor's reason"),
            "`read` has no `None` to give, so `0` is held to the floor: {held}"
        );
    }

    /// A floor whose only failure is zero holds the variable to one unit, and a
    /// value set in code to anything above zero.
    #[test]
    fn a_floor_above_zero_takes_a_sub_unit_value_set_in_code() {
        const BUDGET: DurationBounds = DurationBounds {
            key: "BUDGET_SECS",
            field: "FixtureConfig::budget",
            unit: DurationUnit::Seconds,
            least: Floor::AboveZero("a zero budget gives up before its first attempt"),
            most: Bound {
                count: 3600,
                why: "why",
            },
        };
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
        assert_eq!(sub_unit.value, Duration::from_millis(300));
    }

    #[test]
    fn a_value_from_a_file_is_refused_under_its_file_spelling() {
        let dir =
            std::env::temp_dir().join(format!("nest-rs-config-bounds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("window");
        std::fs::write(&path, "0\n").expect("write");
        let path = path.to_string_lossy().into_owned();
        let text =
            refused(RANGE.read(&env(&[("WINDOW_SECS_FILE", &path)]), Duration::from_secs(5)));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            text.contains(&var_name("fixture", "WINDOW_SECS_FILE")),
            "{text}"
        );
    }
}
