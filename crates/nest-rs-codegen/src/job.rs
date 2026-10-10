//! The worker-job family — `#[process]`, `#[every]`, `#[cron]` and `#[after]` —
//! and the one table of the keys its members take.
//!
//! A key one member takes is answered at every member, by one table, `cell`:
//! each member's [`Grammar`](JobDecorator::grammar) takes its column
//! ([`job_keys`]) and refuses another member's key naming why.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Expr, ExprLit, Lit};

use crate::args::{site, takes_one_of};
use crate::grammar::{Arg, Grammar};
use crate::identity::KEY;
use crate::replicas::REPLICAS;
use crate::ungrouped::ungrouped_expr;

/// The key, spelled once.
pub const TRANSACTIONAL: &str = "transactional";

/// The key, spelled once.
pub const TIMEOUT: &str = "timeout";

/// The longest `timeout` a job decorator takes: a day.
const TIMEOUT_CEILING_MILLIS: u64 = 24 * 60 * 60 * 1000;

/// A member of the worker-job family — a decorator that declares a unit of work
/// a worker transport drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobDecorator {
    /// `#[process]` — a job delivered from a queue.
    Process,
    /// `#[every]` — a tick on a fixed period.
    Every,
    /// `#[cron]` — a tick on a calendar expression.
    Cron,
    /// `#[after]` — a one-shot, a delay after boot.
    After,
}

impl JobDecorator {
    /// Every member, in the order the family is listed.
    pub const ALL: [Self; 4] = [Self::Process, Self::Every, Self::Cron, Self::After];

    /// The attribute as it is written, without `#[…]`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Every => "every",
            Self::Cron => "cron",
            Self::After => "after",
        }
    }

    /// The member written as `name`, `None` for any other attribute.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|member| member.name() == name)
    }

    /// The member's argument grammar: its column of the job-key table, read
    /// through [`Grammar`] like every other decorator's keys.
    pub fn grammar(self) -> Grammar {
        static COLUMNS: [Column; 4] = [
            column(JobDecorator::Process),
            column(JobDecorator::Every),
            column(JobDecorator::Cron),
            column(JobDecorator::After),
        ];
        let at = match self {
            Self::Process => 0,
            Self::Every => 1,
            Self::Cron => 2,
            Self::After => 3,
        };
        let (names, len) = &COLUMNS[at];
        // Non-capturing, so each is a `fn` the grammar holds: the member is a
        // constant in each arm.
        let elsewhere: fn(&str) -> Option<String> = match self {
            Self::Process => |key| job_argument_refused(Self::Process, key),
            Self::Every => |key| job_argument_refused(Self::Every, key),
            Self::Cron => |key| job_argument_refused(Self::Cron, key),
            Self::After => |key| job_argument_refused(Self::After, key),
        };
        let bare: fn(&str, &str) -> String = match self {
            Self::Process => |_, key| job_argument_needs_a_value(Self::Process, key),
            Self::Every => |_, key| job_argument_needs_a_value(Self::Every, key),
            Self::Cron => |_, key| job_argument_needs_a_value(Self::Cron, key),
            Self::After => |_, key| job_argument_needs_a_value(Self::After, key),
        };
        Grammar::new(self.name(), &names[..*len])
            .elsewhere(elsewhere)
            .bare(bare)
    }
}

/// A member's column of the table as names, and how many of them it holds.
type Column = ([&'static str; JobKey::ALL.len()], usize);

/// `member`'s column, computed from the table at compile time.
const fn column(member: JobDecorator) -> Column {
    let mut names = [""; JobKey::ALL.len()];
    let mut len = 0;
    let mut i = 0;
    while i < JobKey::ALL.len() {
        if matches!(cell(JobKey::ALL[i], member), Cell::Takes) {
            names[len] = JobKey::ALL[i].name();
            len += 1;
        }
        i += 1;
    }
    (names, len)
}

/// A key of the worker-job family — every key any member takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKey {
    /// `queue = AudioQueue` — the `#[queue]` marker a job is delivered from.
    Queue,
    /// `retries = 3` — the re-runs a failed attempt gets.
    Retries,
    /// `concurrency = 4` — the attempts one replica runs at once.
    Concurrency,
    /// `throttle(limit = …, window = …)` — the attempts that may start per window.
    Throttle,
    /// `tz = "Europe/Paris"` — the wall clock a calendar is read in.
    Tz,
    /// `transactional = false` — how an attempt's data-layer work is settled.
    Transactional,
    /// `replicas = "one"` — which replicas an occurrence fires on.
    Replicas,
    /// `key = "billing::InvoiceTasks::close_day"` — the identity a job firing
    /// once claims its occurrences under, pinned across a rename.
    Key,
    /// `timeout = "30m"` — how long an attempt runs before it is cut.
    Timeout,
}

impl JobKey {
    /// Every key, in the order a member's column lists them — the order its
    /// unknown-key refusal and its sentences name them in.
    pub const ALL: [Self; 9] = [
        Self::Queue,
        Self::Retries,
        Self::Concurrency,
        Self::Throttle,
        Self::Timeout,
        Self::Tz,
        Self::Transactional,
        Self::Replicas,
        Self::Key,
    ];

    /// The key as it is written.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Retries => "retries",
            Self::Concurrency => "concurrency",
            Self::Throttle => "throttle",
            Self::Timeout => TIMEOUT,
            Self::Tz => "tz",
            Self::Transactional => TRANSACTIONAL,
            Self::Replicas => REPLICAS,
            Self::Key => KEY,
        }
    }

    /// The key written with a value — what a sentence offers the developer to
    /// paste, and what each member's tests prove its parser reads.
    pub const fn example(self) -> &'static str {
        match self {
            Self::Queue => "queue = AudioQueue",
            Self::Retries => "retries = 3",
            Self::Concurrency => "concurrency = 4",
            Self::Throttle => "throttle(limit = 10, window = \"1m\")",
            Self::Timeout => "timeout = \"30m\"",
            Self::Tz => "tz = \"Europe/Paris\"",
            Self::Transactional => "transactional = false",
            Self::Replicas => "replicas = \"one\"",
            Self::Key => "key = \"billing::InvoiceTasks::close_day\"",
        }
    }
}

/// What a member does with a key.
enum Cell {
    /// The member builds the key.
    Takes,
    /// The member cannot take the key, and this is the fact that makes it
    /// meaningless there.
    Refuses(&'static str),
}

/// **The job-key table**: every key against every member, one cell each.
///
/// No arm is a wildcard, so a new key or member does not build without its cells.
const fn cell(key: JobKey, member: JobDecorator) -> Cell {
    use Cell::{Refuses, Takes};
    use JobDecorator::{After, Cron, Every, Process};
    use JobKey::{
        Concurrency, Key, Queue, Replicas, Retries, Throttle, Timeout, Transactional, Tz,
    };
    match (key, member) {
        (Queue, Process) => Takes,
        (Queue, Every | Cron | After) => Refuses(RUNS_IN_PROCESS),

        (Retries, Process) => Takes,
        (Retries, Every | Cron) => Refuses("a tick's retry is the next occurrence"),
        (Retries, After) => Refuses(
            "a one-shot has no next occurrence, so work that must succeed is a queue job it pushes",
        ),

        (Concurrency, Process) => Takes,
        (Concurrency, Every | Cron) => Refuses(NEVER_OVERLAPS),
        (Concurrency, After) => Refuses("a one-shot runs once"),

        (Throttle, Process) => Takes,
        (Throttle, Every | Cron | After) => Refuses("the trigger is the rate"),

        (Tz, Cron) => Takes,
        (Tz, Process) => Refuses("a job runs when delivered, and has no clock to be read in"),
        (Tz, Every) => Refuses("an interval has no wall clock"),
        (Tz, After) => Refuses("a delay has no wall clock"),

        (Timeout, Process | Every | Cron | After) => Takes,

        (Transactional, Process | Every | Cron | After) => Takes,

        (Replicas, Every | Cron) => Takes,
        (Replicas, Process) => {
            Refuses("a job is delivered to one worker, so there is no replica to choose")
        }
        (Replicas, After) => Refuses("a one-shot fires on the replica that booted"),

        (Key, Every | Cron) => Takes,
        (Key, Process) => Refuses(
            "a job is its queue and its own id — the key that keeps a queue from holding one \
             piece of work twice is the push's, `PushOptions::with_unique`",
        ),
        (Key, After) => Refuses(
            "a one-shot fires on the replica that booted and claims nothing, so it has no identity \
             to pin",
        ),
    }
}

/// The one delivery fact all three triggers state: `queue` names where a
/// `#[process]` job is delivered *from*, and a tick is delivered from nowhere.
const RUNS_IN_PROCESS: &str = "a scheduled tick runs in process and is not delivered from a \
     queue — push a job from the tick to reach one";

/// The one overlap fact both recurring triggers state.
const NEVER_OVERLAPS: &str = "a replica runs one occurrence of a scheduled job at a time — one \
     falling due inside a run there is skipped and counted";

/// The keys `member` takes — its column of the table, in [`JobKey::ALL`]'s order.
pub fn job_keys(member: JobDecorator) -> impl Iterator<Item = JobKey> {
    JobKey::ALL
        .into_iter()
        .filter(move |key| matches!(cell(*key, member), Cell::Takes))
}

/// The [`JobKey`] a member's [`Grammar`](JobDecorator::grammar) handed over;
/// a miss is a framework defect, refused through [`unread_job_key`].
pub fn job_key(member: JobDecorator, arg: &Arg<'_>) -> syn::Result<JobKey> {
    JobKey::ALL
        .into_iter()
        .find(|key| key.name() == arg.key())
        .ok_or_else(|| {
            syn::Error::new_spanned(
                arg.ident(),
                format!(
                    "{}: the job-key table holds no such key — a defect in the framework, not \
                     in this code",
                    site(member.name(), Some(arg.key())),
                ),
            )
        })
}

/// The refusal of `key` at `#[member]` when the table refuses it there, naming
/// the fact — `None` for a key this member takes, and for a key no member takes,
/// which the member's [`Grammar`] answers with the unknown-key sentence.
fn job_argument_refused(member: JobDecorator, key: &str) -> Option<String> {
    let key = JobKey::ALL.into_iter().find(|known| known.name() == key)?;
    match cell(key, member) {
        Cell::Takes => None,
        Cell::Refuses(fact) => Some(format!(
            "#[{}] takes no `{}`: {fact}",
            member.name(),
            key.name()
        )),
    }
}

/// The refusal a parser returns for a key its [`Grammar`] handed it and it does
/// not read — the table gives `#[member]` the key, the parser was not taught it.
///
/// A sentence rather than a panic: a proc macro's panic reaches the developer
/// as "proc macro panicked", naming nothing.
pub fn unread_job_key(member: JobDecorator, key: JobKey, at: &impl ToTokens) -> syn::Error {
    syn::Error::new_spanned(
        at,
        format!(
            "{}: the job-key table gives this decorator the key and its parser does not read \
             it — a defect in the framework, not in this code",
            site(member.name(), Some(key.name())),
        ),
    )
}

/// What each value *does*, stated by both refusals below.
const WHAT_THE_VALUES_DO: &str = "`true` (the default) settles the job's data-layer work as one \
     transaction per attempt, so a failed attempt leaves nothing for the retry to repeat; `false` \
     runs it on the pool, for a job that brackets long work that is not the database's";

/// Read a `transactional = …` value written at `#[member]` off the expression
/// the key was given.
///
/// The refusal states what each value *does*, not the type it wanted.
pub fn transactional_value(member: JobDecorator, expr: &Expr) -> syn::Result<bool> {
    match ungrouped_expr(expr) {
        Expr::Lit(ExprLit {
            lit: Lit::Bool(b), ..
        }) => Ok(b.value()),
        other => Err(syn::Error::new_spanned(
            other,
            format!(
                "{} — {WHAT_THE_VALUES_DO}",
                takes_one_of(member.name(), TRANSACTIONAL, &["true", "false"])
            ),
        )),
    }
}

/// Read a `timeout = "…"` value written at `#[member]`, in milliseconds: a
/// whole number of `ms`, `s`, `m` or `h`, above zero and at most a day.
pub fn timeout_value(member: JobDecorator, expr: &Expr) -> syn::Result<u64> {
    let millis = crate::duration_millis(member.name(), Some(TIMEOUT), expr)?;
    if millis > TIMEOUT_CEILING_MILLIS {
        return Err(syn::Error::new_spanned(
            ungrouped_expr(expr),
            format!(
                "{} is at most `\"24h\"`: an attempt running past a day is a process of its own \
                 — split it into jobs that each end within one",
                site(member.name(), Some(TIMEOUT)),
            ),
        ));
    }
    Ok(millis)
}

/// The deadline a parsed `timeout` sets, rooted at the surface path re-exporting
/// `nest_rs_worker` that the calling macro emits through; `None` is
/// `nest_rs_worker::JOB_TIMEOUT`.
pub fn job_timeout(millis: Option<u64>, surface: &TokenStream) -> TokenStream {
    match millis {
        Some(millis) => quote! { ::core::time::Duration::from_millis(#millis) },
        None => quote! { #surface::nest_rs_worker::JOB_TIMEOUT },
    }
}

/// The refusal for the key written **bare**, with no value.
fn transactional_needs_a_value(member: JobDecorator) -> String {
    format!(
        "{} needs a value — write `{TRANSACTIONAL} = true` or `{TRANSACTIONAL} = false`. \
         {WHAT_THE_VALUES_DO}",
        site(member.name(), Some(TRANSACTIONAL)),
    )
}

/// The refusal for **any** key of a job decorator written bare — the shared
/// sentence, or the `transactional` one when that is the key. `name` is the key
/// as the member spells it, nested ones included (`throttle(limit)`).
pub fn job_argument_needs_a_value(member: JobDecorator, name: &str) -> String {
    if name == TRANSACTIONAL {
        transactional_needs_a_value(member)
    } else {
        crate::args::needs_a_value(member.name(), name)
    }
}

/// The refusal of a job method answering `()` — written or not — naming what its
/// `Result` decides.
pub fn job_returns_a_result(member: JobDecorator) -> String {
    let outcome = match member {
        JobDecorator::Process => {
            "`Ok(())` completes the job and an `Err` fails the attempt, which the retry budget \
             decides on"
        }
        JobDecorator::Every | JobDecorator::Cron | JobDecorator::After => {
            "`Ok(())` completes the occurrence and an `Err` fails it, which the scheduler logs \
             before the next one"
        }
    };
    format!(
        "a `#[{}]` method returns a `Result`: {outcome} — write `-> anyhow::Result<()>`",
        member.name()
    )
}

/// The `JobTransaction` variant a parsed value selects, rooted at the surface
/// path re-exporting `nest_rs_worker` that the calling macro emits through
/// (`::nest_rs_queue::__private`, `::nest_rs_schedule`).
///
/// `None` — the key was not written — is `PerAttempt`, spelled out.
pub fn job_transaction(value: Option<bool>, surface: &TokenStream) -> TokenStream {
    match value {
        Some(false) => quote! { #surface::nest_rs_worker::JobTransaction::Pool },
        Some(true) | None => quote! { #surface::nest_rs_worker::JobTransaction::PerAttempt },
    }
}

#[cfg(test)]
mod tests {
    use proc_macro2::Span;

    use super::*;

    fn read(member: JobDecorator, name: &str) -> Result<JobKey, String> {
        let at = syn::Ident::new(name, Span::call_site());
        let mut read = None;
        member
            .grammar()
            .parse2(quote!(#at), |arg| {
                read = Some(job_key(member, &arg)?);
                Ok(())
            })
            .map_err(|refusal| refusal.to_string())?;
        read.ok_or_else(|| "nothing read".to_owned())
    }

    #[test]
    fn every_key_is_taken_by_some_member() {
        for key in JobKey::ALL {
            assert!(
                JobDecorator::ALL
                    .into_iter()
                    .any(|member| job_keys(member).any(|taken| taken == key)),
                "no member takes `{}`",
                key.name(),
            );
        }
    }

    #[test]
    fn each_cell_is_read_as_the_table_states_it() {
        for member in JobDecorator::ALL {
            for key in JobKey::ALL {
                let read = read(member, key.name());
                match cell(key, member) {
                    Cell::Takes => assert_eq!(read, Ok(key)),
                    Cell::Refuses(fact) => assert_eq!(
                        read,
                        Err(format!(
                            "#[{}] takes no `{}`: {fact}",
                            member.name(),
                            key.name()
                        )),
                    ),
                }
            }
        }
    }

    #[test]
    fn a_key_no_member_takes_is_unknown_and_lists_the_members_column() {
        assert_eq!(
            read(JobDecorator::Cron, "priority"),
            Err(
                "unknown #[cron] argument `priority`; expected `timeout`, `tz`, `transactional`, \
                 `replicas` or `key`"
                    .to_owned()
            ),
        );
    }

    #[test]
    fn a_timeout_is_read_in_milliseconds_above_zero_and_within_a_day() {
        let value = |literal: &str| -> Expr { syn::parse_str(literal).expect("an expression") };
        for member in JobDecorator::ALL {
            assert_eq!(
                timeout_value(member, &value("\"30m\"")).ok(),
                Some(1_800_000)
            );
            assert_eq!(
                timeout_value(member, &value("\"24h\"")).ok(),
                Some(86_400_000)
            );
            assert!(timeout_value(member, &value("\"0s\"")).is_err());
            let past = timeout_value(member, &value("\"25h\""))
                .expect_err("a deadline past a day")
                .to_string();
            assert!(
                past.contains(&format!("#[{}] `timeout`", member.name())) && past.contains("24h"),
                "{past}"
            );
        }
    }

    #[test]
    fn a_member_is_found_by_the_name_it_is_written_with() {
        for member in JobDecorator::ALL {
            assert_eq!(JobDecorator::named(member.name()), Some(member));
        }
        assert_eq!(JobDecorator::named("scheduled"), None);
    }
}
