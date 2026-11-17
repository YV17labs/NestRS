# A line filed while another is being written is written: the hook asks a fence

Settled on 2026-11-17, from item 13 of the `@nestjs/observe` audit, the owner
handing the prompt.

Since 2026-10-18 (`06494d1e6`, a panic inside a subscriber's own dispatch is one
stderr line), the kernel set one scoped default, on a thread that ends at once,
for the life of the process, the first time a subscriber was set. tracing-core
then stays on its per-thread path (`get_default`, tracing-core 0.1.36), where an
event filed while the thread is inside `event` or `new_span` is handed
`Dispatch::none()`: the hook's own event no longer re-entered a subscriber
holding a lock while it wrote a field whose `Display` panicked. That is the one
question tracing-core answers about re-entrance, and it answers it by discarding
everything filed there, in every app, without a word. Seen, each red before the
change: OpenTelemetry's `BatchLogProcessor.LogDroppingStarted`, filed from the
log bridge's `on_event`, never reached the console; a line a layer files from its
`on_event` did not either, nor one a field's `Display` files.

- **Every subscriber the framework installs is wrapped in `Fenced`**
  (`crates/nest-rs-core/src/fenced.rs`), by `install_global_subscriber`: the
  console an `App` installs, `install_console`'s, OpenTelemetry's. It hands every
  method of `Subscriber` to the one it wraps — `on_register_dispatch`,
  `current_span` and `downcast_raw` included, since the trace layer and the
  linker downcast the global dispatch — and counts, per thread, how many of them
  the thread is inside, through a guard dropped on unwind too.
- **The hook asks the fence**: `panic::subscribed()` answers `false` while this
  thread is inside, so the hook writes its stderr line there, as before. With a
  fenced subscriber installed no scoped default is set, and tracing-core keeps
  its fast path, which delivers a line filed inside.
- **The fence covers what tracing-core's path never did**: `record`, `enter`,
  `exit` and `try_close`, which a span calls on its own dispatch, past
  `get_default`. Seen: a panic in a field recorded through `Span::record` under a
  locking subscriber hung the process (a child killed at 20 s); it is one stderr
  line now.
- **A subscriber the app sets itself before its boot keeps the hold**, from
  `App::build`, since nothing can wrap it: a line filed inside its dispatch is
  still discarded, and a panic in its `record` still waits on its lock (seen, a
  child killed at 20 s). Held by a child-process test for the event's case.
- **Cost: none T1 can tell.** The benchmark's T1 (`BENCH_TIERS=t1`, the
  published protocol, the SUT on one core), alternated base and head twice in
  one sitting, read medians of 164 488, 181 662, 176 272 and 177 245 requests a
  second before, and 161 940, 146 452, 177 618 and 178 606 after. The launches
  whose runs held within 5 % — 177 245 before, 177 618 and 178 606 after — put
  the head level with the base. One head launch ran below every base launch, its
  runs within 5.3 %; the two after it did not repeat it. The fence adds two
  thread-local writes to each subscriber call and takes back tracing-core's fast
  path, which the hold had taken away.

What it leaves, read and not seen: tracing-subscriber 0.3.23 keeps the
per-layer filters' verdict for the event being dispatched in one per-thread
`FilterState`, and an event filed inside that goes through an `enabled` pass
rewrites it. The framework's three per-layer filters agree on every event, and a
callsite their static directives settle takes no such pass; a span-scoped
directive (`target[span]=level`) makes one, and an event filed inside that every
filter refuses would leave the later layers skipping the event being written.

Refused:

- **Removing the hold with no fence**: the hook's event re-enters a locking
  subscriber and the process hangs (the three child-process tests that predate
  this).
- **Fencing the hook by a timer, or filing its event from a helper thread**: a
  timer guesses how long a layer holds its lock, and a helper thread waits on
  the same lock or files out of order.
- **Filing a line of our own in place of the SDK's**: the SDK's is the line, and
  it now arrives.
- **A fence holding a `Dispatch`**: tracing-core keeps a dispatch's
  `downcast_raw` crate-private, so such a fence could hand no downcast on; the
  installer takes the subscriber's type instead.
- **A fence in the console's layer alone**: OpenTelemetry's subscriber, and a
  layer an app sets beside the console, would stay outside it.
