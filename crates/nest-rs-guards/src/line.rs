//! The one line a guard's denial files, said by the edge that ran the guard on
//! the target the guard declares ([`Guard::target`](crate::Guard::target)).

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock, PoisonError, RwLock};

use nest_rs_core::UnitContext;
use tracing_core::callsite::{self, Callsite};
use tracing_core::field::{Field, FieldSet, Value};
use tracing_core::metadata::{Kind, Level, LevelFilter, Metadata};
use tracing_core::subscriber::Interest;
use tracing_core::{Event, identify_callsite};

use crate::Denial;
use crate::denial::failure_chain;

/// Every field a denial line carries, in the order its values are given.
const FIELDS: [&str; 10] = [
    "message",
    "guard",
    "unit",
    "host",
    "address",
    "conn_id",
    "status",
    "reason",
    "retry_after",
    "error",
];

/// A denial line's callsite: what `tracing`'s macros declare as a `static`,
/// made here once per target, message and level, since a target is a
/// callsite's constant and the guard names its own at run time.
struct Line {
    metadata: OnceLock<Metadata<'static>>,
    fields: OnceLock<[Option<Field>; FIELDS.len()]>,
}

impl Callsite for Line {
    // Nothing cached: each denial asks the dispatcher, a denial being rare
    // beside an admission.
    fn set_interest(&self, _interest: Interest) {}

    fn metadata(&self) -> &Metadata<'_> {
        #[expect(
            clippy::expect_used,
            reason = "set before the line is registered, the only way anything reaches it"
        )]
        self.metadata
            .get()
            .expect("a denial line's metadata is set when the line is made")
    }
}

/// The lines made so far, by target, message and level: a set bounded by the
/// guards an app runs, the edges' messages and two levels.
type Lines = HashMap<(&'static str, &'static str, Level), &'static Line>;

static LINES: LazyLock<RwLock<Lines>> = LazyLock::new(RwLock::default);

/// The line `message` is filed as on `target` at `level`, made on first use.
fn line(target: &'static str, message: &'static str, level: Level) -> &'static Line {
    let key = (target, message, level);
    if let Some(line) = LINES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return line;
    }
    let mut lines = LINES.write().unwrap_or_else(PoisonError::into_inner);
    lines.entry(key).or_insert_with(|| {
        // Leaked as a `static` callsite lives for the process: one per key.
        let line: &'static Line = Box::leak(Box::new(Line {
            metadata: OnceLock::new(),
            fields: OnceLock::new(),
        }));
        let fields = FieldSet::new(&FIELDS, identify_callsite!(line));
        line.fields
            .get_or_init(|| std::array::from_fn(|index| fields.iter().nth(index)));
        line.metadata.get_or_init(|| {
            Metadata::new(
                concat!("event ", file!(), ":", line!()),
                target,
                level,
                Some(file!()),
                Some(line!()),
                Some(module_path!()),
                fields,
                Kind::EVENT,
            )
        });
        callsite::register(line);
        line
    })
}

/// File `denial`, raised by the guard named `guard` on the unit `cx`, as
/// `message` on the guard's `target`: at `error` for a `500`, a wiring defect,
/// else at `warn`. The line names the guard, the unit
/// (`http.request`, `ws.message`, …), its host and address and the connection
/// it arrived on (`conn_id`, as every WebSocket line names it), the status, the
/// denial's reason and `Retry-After`, and the chain behind a denial keeping a
/// cause — never a denial's own text otherwise, which may name what no line
/// carries.
pub(crate) fn file_denial(
    message: &'static str,
    guard: &'static str,
    target: &'static str,
    cx: &UnitContext<'_>,
    denial: &Denial,
) {
    // A `503` is the server unable to serve in time, filed at `warn` as every
    // edge files one; a `500` is a defect.
    let level = match denial.http_status() {
        500 => Level::ERROR,
        _ => Level::WARN,
    };
    if level > LevelFilter::current() {
        return;
    }
    let line = line(target, message, level);
    let Some(
        [
            Some(said),
            Some(named),
            Some(unit),
            Some(host),
            Some(address),
            Some(conn_id),
            Some(status),
            Some(reason),
            Some(retry_after),
            Some(error),
        ],
    ) = line.fields.get()
    else {
        return;
    };
    let message = tracing_core::field::display(message);
    let unit_name = cx.unit().name();
    let host_name = cx.handler().host_name();
    let addressed = cx.handler().address();
    let arrived_on = cx.peer().and_then(|peer| peer.connection);
    let code = denial.http_status();
    let why = denial.reason();
    let wait = denial.retry_after_secs();
    let chain = failure_chain(denial);
    // An absent value is a `None`, which records nothing, as the macros' do.
    let values: [(&Field, Option<&dyn Value>); FIELDS.len()] = [
        (said, Some(&message)),
        (named, Some(&guard)),
        (unit, Some(&unit_name)),
        (host, Some(&host_name)),
        (address, Some(&addressed)),
        (conn_id, Some(&arrived_on)),
        (status, Some(&code)),
        (reason, Some(&why)),
        (retry_after, Some(&wait)),
        (error, Some(&chain)),
    ];
    let metadata = line.metadata();
    // `value_set` is how an event outside a macro is given its values: the
    // way `tracing-log` dispatches a record.
    Event::dispatch(metadata, &metadata.fields().value_set(&values));
}
