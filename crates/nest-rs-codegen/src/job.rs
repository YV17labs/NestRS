//! The worker-job family — `#[process]`, `#[every]`, `#[cron]` and `#[after]` —
//! and the one table of the keys its members take.
//!
//! All four declare a unit of work a worker transport drives, so a key one member
//! takes is answered at every member: built where it means something, refused
//! where it cannot, naming the fact that makes it meaningless there. That answer
//! is one table, `cell`, and it is the only statement of it: every member reads
//! its keys through [`job_key`], the refusal of a key another member takes reads
//! the same cell, and the list an unknown key is told about is the member's column
//! ([`job_keys`]). A key added to a parser and not to the table does not compile —
//! the parser matches on [`JobKey`], whose every variant the table must place at
//! every member — and a key the table gives a member its parser does not read
//! fails that parser's own tests.
//!
//! `transactional` is the one key every member takes: every worker job runs
//! through the one `JobContext` seam, so there is no site that cannot. Its value
//! is read here too, so a bad one reads the same wherever it is typed.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Expr, ExprLit, Lit};

use crate::args::{WrittenKeys, site, takes_one_of};
use crate::replicas::REPLICAS;
use crate::ungrouped::ungrouped_expr;

/// The key, spelled once.
pub const TRANSACTIONAL: &str = "transactional";

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
}

impl JobKey {
    /// Every key, in the order a member's column lists them — the order its
    /// unknown-key refusal and its sentences name them in.
    pub const ALL: [Self; 7] = [
        Self::Queue,
        Self::Retries,
        Self::Concurrency,
        Self::Throttle,
        Self::Tz,
        Self::Transactional,
        Self::Replicas,
    ];

    /// The key as it is written.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Retries => "retries",
            Self::Concurrency => "concurrency",
            Self::Throttle => "throttle",
            Self::Tz => "tz",
            Self::Transactional => TRANSACTIONAL,
            Self::Replicas => REPLICAS,
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
            Self::Tz => "tz = \"Europe/Paris\"",
            Self::Transactional => "transactional = false",
            Self::Replicas => "replicas = \"one\"",
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
/// A `match` rather than a list, because a list is closed only by a test and a
/// match by the compiler: a key or a member added without a cell at every
/// crossing does not build, and a cell written twice is an unreachable pattern.
/// No arm is a wildcard, for the same reason — `transactional` names all four
/// members rather than `_`, so a fifth member states its own answer.
const fn cell(key: JobKey, member: JobDecorator) -> Cell {
    use Cell::{Refuses, Takes};
    use JobDecorator::{After, Cron, Every, Process};
    use JobKey::{Concurrency, Queue, Replicas, Retries, Throttle, Transactional, Tz};
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

        (Transactional, Process | Every | Cron | After) => Takes,

        (Replicas, Every | Cron) => Takes,
        (Replicas, Process) => {
            Refuses("a job is delivered to one worker, so there is no replica to choose")
        }
        (Replicas, After) => Refuses("a one-shot fires on the replica that booted"),
    }
}

/// The one delivery fact all three triggers state: `queue` names where a
/// `#[process]` job is delivered *from*, and a tick is delivered from nowhere.
const RUNS_IN_PROCESS: &str = "a scheduled tick runs in process and is not delivered from a \
     queue — push a job from the tick to reach one";

/// The one overlap fact both recurring triggers state.
const NEVER_OVERLAPS: &str = "a scheduled job never overlaps itself — an occurrence falling \
     inside a run is skipped and counted";

/// The keys `member` takes — its column of the table, in [`JobKey::ALL`]'s order.
pub fn job_keys(member: JobDecorator) -> impl Iterator<Item = JobKey> {
    JobKey::ALL
        .into_iter()
        .filter(move |key| matches!(cell(*key, member), Cell::Takes))
}

/// Read a key written at `#[member]` against the table, spanned at `at`, taking
/// it through `written` — the declaration's [`WrittenKeys`].
///
/// The key when the member takes it; otherwise one of three refusals, and the
/// order is the point. A key **another member** takes is not misspelled here, it
/// is meaningless, so it is refused naming why (`job_argument_refused`) —
/// checked first and whatever the value, so a developer carrying `retries` from a
/// `#[process]` to an `#[every]` learns what a tick does instead. A key **no
/// member** takes keeps the unknown-key sentence, listing the member's column,
/// and a key written twice the repeat sentence — both through [`WrittenKeys`],
/// so the whole family refuses a repeat for every key of its column.
pub fn job_key(
    member: JobDecorator,
    written: &mut WrittenKeys,
    name: &str,
    at: &impl ToTokens,
) -> syn::Result<JobKey> {
    if let Some(refusal) = job_argument_refused(member, name) {
        return Err(syn::Error::new_spanned(at, refusal));
    }
    let column: Vec<JobKey> = job_keys(member).collect();
    let names: Vec<&str> = column.iter().map(|key| key.name()).collect();
    let position = written.take_key(member.name(), &names, at, name)?;
    Ok(column[position])
}

/// The refusal of `key` at `#[member]` when the table refuses it there, naming
/// the fact — `None` for a key this member takes, and for a key no member takes,
/// which [`job_key`] answers with the unknown-key sentence.
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

/// The refusal a parser returns for a key [`job_key`] handed it and it does not
/// read — the table gives `#[member]` the key, the parser was not taught it.
///
/// A framework defect, never the developer's: each member's tests read every key
/// of its column, so this sentence fails a test before it can reach a build. It
/// is a sentence rather than a panic because a proc macro's panic reaches the
/// developer as "proc macro panicked", naming nothing.
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

/// What each value *does* — the half of both refusals below that a developer is
/// actually choosing between, and the reason both are worded here: one key, four
/// decorators, one sentence.
const WHAT_THE_VALUES_DO: &str = "`true` (the default) settles the job's data-layer work as one \
     transaction per attempt, so a failed attempt leaves nothing for the retry to repeat; `false` \
     runs it on the pool, for a job that brackets long work that is not the database's";

/// Read a `transactional = …` value written at `#[member]` off the expression
/// the key was given.
///
/// The sentence names the decorator, as every value refusal does
/// ([`crate::args::site`]), and then what each value *does* rather than the type
/// it wanted: a developer reaching for this key is choosing between two
/// behaviours, not fixing a typo, and the choice is the thing worth stating at
/// the point of refusal.
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

/// The refusal for the key written **bare**, with no value.
///
/// A separate sentence from the one above, because the two mistakes differ: a
/// wrong value is a choice mis-typed, a missing one is a declaration that says
/// nothing. What both sites owed and neither gave was the second half — a bare
/// `expected =` names the grammar and not the key, and on a trigger it did not
/// even land on the right decorator, the argument list parsing through a
/// `Punctuated` whose failure is reported at the enclosing `#[scheduled]`.
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
///
/// One call rather than the `if name == TRANSACTIONAL { … } else { … }` both job
/// decorators had written out: the branch is the shared thing, not just its two
/// arms.
pub fn job_argument_needs_a_value(member: JobDecorator, name: &str) -> String {
    if name == TRANSACTIONAL {
        transactional_needs_a_value(member)
    } else {
        crate::args::needs_a_value(member.name(), name)
    }
}

/// The refusal of a job method answering `()` — written or not — naming what its
/// `Result` decides. `#[process]` and the three triggers state it one way; only
/// what an `Err` does differs, because a job has a retry budget and an occurrence
/// has the next one.
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
/// crate the calling macro emits through (`::nest_rs_queue`, `::nest_rs_schedule`).
///
/// `None` — the key was not written — is `PerAttempt`, spelled out rather than
/// left to `Default` so the expansion states which behaviour it chose.
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
        let at = syn::Ident::new("at", Span::call_site());
        job_key(member, &mut WrittenKeys::default(), name, &at)
            .map_err(|refusal| refusal.to_string())
    }

    /// A key no member takes is dead vocabulary rather than a family key: it
    /// would be refused everywhere, as a word the family claims.
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

    /// Every crossing of the table reaches the developer as its cell says: the
    /// key where it is taken, the family's sentence where it is refused.
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

    /// A key no member takes is unknown, and the sentence lists the member's
    /// column — the table's, in its order.
    #[test]
    fn a_key_no_member_takes_is_unknown_and_lists_the_members_column() {
        assert_eq!(
            read(JobDecorator::Cron, "priority"),
            Err(
                "unknown #[cron] argument `priority`; expected `tz`, `transactional` or `replicas`"
                    .to_owned()
            ),
        );
    }

    #[test]
    fn a_member_is_found_by_the_name_it_is_written_with() {
        for member in JobDecorator::ALL {
            assert_eq!(JobDecorator::named(member.name()), Some(member));
        }
        assert_eq!(JobDecorator::named("scheduled"), None);
    }
}
