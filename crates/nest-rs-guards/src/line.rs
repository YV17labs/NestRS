//! The one line a guard's denial files, said by the edge that ran the guard on
//! the target the guard declares ([`Guard::target`](crate::Guard::target)).

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock, PoisonError, RwLock};

use nest_rs_core::__private::shorten;
use nest_rs_core::UnitContext;
use tracing_core::callsite::{self, Callsite};
use tracing_core::field::{Field, FieldSet, Value};
use tracing_core::metadata::{Kind, Level, LevelFilter, Metadata};
use tracing_core::subscriber::Interest;
use tracing_core::{Event, dispatcher, identify_callsite};

use crate::Denial;
use crate::denial::failure_chain;

/// Every field a denial line carries, in the order its values are given.
const FIELDS: [&str; 19] = [
    "message",
    "guard",
    "unit",
    "host",
    "method",
    "verb",
    "path",
    "kind",
    "field",
    "event",
    "tool",
    "prompt",
    "resource",
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
    // Nothing cached: each denial asks the dispatcher whether its line is
    // enabled (`file_denial`), a denial being rare beside an admission.
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
/// else at `warn`. The line names the guard as an app writes it, the unit
/// (`http.request`, `ws.message`, …), its host, its method and where it is
/// served, by the fields the unit's `mounted` line names it by
/// ([`ServedAddress`](nest_rs_core::ServedAddress)) or the handler's `address`
/// on a view stating none, the connection it arrived on (`conn_id`, as every
/// WebSocket line names it), the status, the denial's reason and `Retry-After`,
/// and the chain behind a denial keeping a cause — never a denial's own text
/// otherwise, which may name what no line carries.
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
            Some(method),
            Some(verb),
            Some(path),
            Some(kind),
            Some(field),
            Some(event),
            Some(tool),
            Some(prompt),
            Some(resource),
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
    let metadata = line.metadata();
    // An event made outside the macros asks the dispatcher itself whether it is
    // enabled, as `tracing-log` dispatches a record: `Event::dispatch` alone
    // skips every per-target filter.
    dispatcher::get_default(|dispatch| {
        if !dispatch.enabled(metadata) {
            return;
        }
        let message = tracing_core::field::display(message);
        let guard = shorten(guard);
        let guard = guard.as_str();
        let unit_name = cx.unit().name();
        let handler = cx.handler();
        let host_name = handler.host_name();
        // A framework endpoint dispatches to no method of its own.
        let dispatched = Some(handler.method()).filter(|method| !method.is_empty());
        let served = cx.served_address();
        let declared = served.is_none().then(|| handler.address());
        let served = served.unwrap_or_default();
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
            (method, Some(&dispatched)),
            (verb, Some(&served.verb)),
            (path, Some(&served.path)),
            (kind, Some(&served.kind)),
            (field, Some(&served.field)),
            (event, Some(&served.event)),
            (tool, Some(&served.tool)),
            (prompt, Some(&served.prompt)),
            (resource, Some(&served.resource)),
            (address, Some(&declared)),
            (conn_id, Some(&arrived_on)),
            (status, Some(&code)),
            (reason, Some(&why)),
            (retry_after, Some(&wait)),
            (error, Some(&chain)),
        ];
        dispatch.event(&Event::new(metadata, &metadata.fields().value_set(&values)));
    });
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::sync::{Arc, Mutex};

    use nest_rs_core::__private::{
        HandlerDeclaration, MetaLevels, declare_handler, declare_unit, log_filter, new_unit_context,
    };
    use nest_rs_core::operation_log::Kind;
    use nest_rs_core::{Handler, Posture, ServedAddress, UnitView};
    use tracing_subscriber::Registry;
    use tracing_subscriber::layer::{Context, SubscriberExt as _};

    use super::*;

    struct Socket;

    impl UnitView for Socket {}

    struct Host;

    static MESSAGE: Handler = declare_handler(HandlerDeclaration {
        unit: declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message"),
        host: TypeId::of::<Host>(),
        host_name: "Host",
        method: "send",
        address: "send",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    /// The target of every line a subscriber writes.
    #[derive(Clone, Default)]
    struct Written(Arc<Mutex<Vec<&'static str>>>);

    impl<S: tracing_core::Subscriber> tracing_subscriber::Layer<S> for Written {
        fn on_event(&self, event: &Event<'_>, _cx: Context<'_, S>) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event.metadata().target());
        }
    }

    /// An operator silencing a guard's target silences its denials, as it
    /// silences every line on it; the filter's other targets still write theirs.
    #[test]
    fn a_denial_line_on_a_target_the_filter_silences_is_not_written() {
        let written = Written::default();
        let subscriber = Registry::default()
            .with(log_filter("warn,acme::silenced=off"))
            .with(written.clone());
        let mut view = Socket;
        let cx = new_unit_context(&MESSAGE, &mut view, None, None);

        tracing::subscriber::with_default(subscriber, || {
            for target in ["acme::silenced", "acme::heard"] {
                file_denial(
                    "guard denied the message",
                    "Door",
                    target,
                    &cx,
                    &Denial::forbidden("closed"),
                );
            }
        });

        let written = written.0.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(*written, ["acme::heard"]);
    }

    /// A view stating no address has the handler's said: a package's unit is
    /// still named, by what its decorator declares.
    #[test]
    fn a_view_stating_no_address_has_the_handlers_said() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut view = Socket;
        let cx = new_unit_context(&MESSAGE, &mut view, None, None);

        file_denial(
            "guard denied the message",
            "acme::door::Door<acme::keys::Key>",
            "acme::door",
            &cx,
            &Denial::forbidden("closed"),
        );

        let line = logs.expect_one("acme::door", "guard denied the message");
        assert_eq!(line.field("guard").as_deref(), Some("Door<Key>"));
        assert_eq!(line.field("host").as_deref(), Some("Host"));
        assert_eq!(line.field("method").as_deref(), Some("send"));
        assert_eq!(line.field("address").as_deref(), Some("send"));
        assert_eq!(line.field("event"), None);
    }

    struct Stating;

    impl UnitView for Stating {
        fn served_address(&self) -> Option<ServedAddress<'_>> {
            Some(ServedAddress::event("said"))
        }
    }

    static MOUNT: Handler = declare_handler(HandlerDeclaration {
        unit: declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message"),
        host: TypeId::of::<Host>(),
        host_name: "Host",
        method: "",
        address: "/mount",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    /// A view stating its address has it said in place of the handler's, and a
    /// framework endpoint, which dispatches to no method, names none.
    #[test]
    fn a_stated_address_replaces_the_handlers_and_an_endpoint_names_no_method() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut view = Stating;
        let cx = new_unit_context(&MOUNT, &mut view, None, None);

        file_denial(
            "guard denied the message",
            "Door",
            "acme::door",
            &cx,
            &Denial::forbidden("closed"),
        );

        let line = logs.expect_one("acme::door", "guard denied the message");
        assert_eq!(line.field("event").as_deref(), Some("said"));
        assert_eq!(line.field("address"), None);
        assert_eq!(line.field("method"), None);
    }
}
