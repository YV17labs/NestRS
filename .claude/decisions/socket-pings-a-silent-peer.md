# A socket pings a silent peer, and drops one that sends nothing back

Settled on 2026-11-07, closing the vanished-peer line `TODO.md` carried since
B-213: neither socket edge sent a Ping, so a peer gone without a close (a phone
off its network, a laptop shut mid-session) held its socket, its task, its
registry entry and its place under the connection cap until the connection's
ceiling, four hours by default. Its TCP connection reports nothing: with nothing
to send, the kernel never learns the peer is gone. Security and performance
decided, and RFC 6455 gives the means (§5.5.2: "a Ping frame may serve either as
a keepalive or as a means to verify that the remote endpoint is still
responsive").

What both sockets do, a gateway's and a graphql-ws one's, through the one
`KeepAlive` beside the close codes in `nest-rs-http`'s WebSocket module:

- **A Ping once the peer has sent nothing for the interval**, counted from its
  last frame of any kind. A socket in use is never pinged, so the cost is paid by
  idle sockets alone: one timer per socket, which moves when it fires and when a
  Ping is answered, so a timeout longer than the interval never holds the next
  Ping back; a frame costs a clock read.
- **Any frame back is the answer**, its Pong or anything else. A peer that sends
  nothing within the timeout of the Ping is dropped, so a vanished one is released
  within the interval and the timeout of its last frame.
- **The drop sends no Close.** §7.1.7 says an endpoint failing a connection
  *should* send one and *may* omit it "if it believes the other side is unlikely
  to be able to receive and process it": a peer that answered nothing for the
  timeout is that case, and no §7.4.1 code says "you went silent" (1011 claims a
  server fault, 1001 a server going away). The peer reads 1006, the code for a
  connection that ended without a Close, which is what happened. The gateway's
  disconnect hook runs as on any end; the subscription socket files `cancelled`,
  as for the send deadline's drop, its sibling.
- **One `warn` per drop**, as the send deadline's, naming the mount by `host` and
  `path`, never the peer.
- **Two settings per edge**, `ping_interval` (`Option`, `0` off) and
  `ping_timeout` (never off: a Ping nobody waits for releases nothing, so a
  deployment that wants none turns the interval off), read as
  `<PREFIX>_WS__PING_*_SECS` and `<PREFIX>_GRAPHQL__PING_*_SECS`, on one range in
  `nest-rs-http` beside the other socket bounds. Named for RFC 6455's frame, as
  Socket.IO (NestJS's gateway default) and Python's `websockets` name theirs.
- **20 seconds each by default**: under the sixty-second idle timeout of nginx and
  of a cloud load balancer, so the Pings also keep a quiet socket through a proxy;
  the interval and timeout of this crate's own HTTP/2 Ping and of `websockets`; a
  vanished peer is released within 40 seconds.

**Refused:**

- **graphql-transport-ws's `ping` message on the subscription socket.** It needs
  a client library that answers it, and the legacy `graphql-ws` subprotocol has
  none; a WebSocket Ping is answered by every client as it reads, a browser's
  `WebSocket` included, whatever the subprotocol.
- **A Ping every interval whatever the traffic** (Socket.IO, `websockets`). It
  pays a timer and a frame on every busy socket to learn what the busy socket's
  own frames already said.
- **A Close frame with a code before the drop.** No code fits, and the frame is a
  write to a peer presumed gone, which can only wait on a full buffer.
- **`ping_timeout` as an `Option`.** Pings without a timeout keep a proxy open and
  release no vanished peer, the half this exists for.
- **One setting shared by both edges.** Each edge runs its own loop and reads its
  own namespace, as the connection ceiling does.

Not proved over a real socket: that an answer keeps a socket. Paused time
outruns a Pong crossing the kernel's loopback, so a suite would read it as
silence; it is proved beside each loop over an in-memory pipe (the gateway's
connection loop, the subscription socket's peer half), and the drop over a real
socket in `nest-rs-testing`'s `silent_peer.rs`. A peer that vanished, its packets
dropped, needs a network the suites do not have: what the server sees of one, a
peer that sends nothing back, is what they prove.

**2026-11-07, two orderings the keepalive owes** (found by this branch's review,
each held by a test beside its loop on an in-memory pipe). A peer is pinged by
its own silence, not its socket's: one that only listens to a busy feed is
pinged like an idle one, so the Pings' cost is paid by every socket whose peer
sends nothing.

- **The gateway's writer sends the Ping ahead of the frames queued before it.**
  Behind them, it waited for an outbox that never emptied, and the peer, never
  asked, was dropped as silent. At most one Ping per interval goes first, so the
  replies are never held back by it. graphql-ws's loop picks between the Ping
  and the engine's next frame at random, which delays the Ping by a frame or two
  and never for good: left as it is.
- **A frame waiting unread when the Ping's wait ends is its answer.** Both
  sockets read what waits before they judge the peer silent, so a loop busy
  writing as the wait ends never drops a peer that answered in time; the
  subscription socket's peer half judged first.

**2026-11-08, the Ping reaches the writer without a lock** (found by this
branch's review). The writer polls the Ping's wake-up beside every frame it
writes: a `tokio::sync::Notify` registered each wait under its waiters' lock
and took the lock again to drop it, two round trips a pushed frame for an event
due once per interval. The wake-up is a `Signal` in `nest_rs_http`'s socket
ground, a flag and an `AtomicWaker`, a few atomic operations a poll; a writer
whose Pings are off polls nothing for it. graphql-ws's withheld `subscribe`
refusal wakes the same loop, and takes the same signal.

- Refused: a `Notified` pinned across frames, which still takes the lock on
  each poll to refresh its waker; an `mpsc(1)` arm, lock-free too, whose full
  channel a raise would have to set aside as no error.

**2026-11-11, an MCP session's client is pinged on its stream** (closing the
`TODO.md` line "an MCP event stream carries no frame its client answers"). A
client gone without a close while holding its session's `GET` stream kept its
session — a worker, its handler, a place under the cap — to the idle timeout, and
past it while the server wrote to it: rmcp's session worker re-arms its idle timer
on every message, either way. Seen by `nest-rs-mcp`'s `ping` suite on paused
time: a silent client's stream carried nothing and lived to the idle timeout.

- **MCP's own `ping` request** (the specification's *Ping* utility, which both
  sides must answer promptly), once the client has sent nothing for
  `ping_interval`, and the session ended when nothing comes back within
  `ping_timeout`: the sockets' rule, their ranges and their defaults, read as
  `<PREFIX>_MCP__PING_*_SECS`. `KeepAlive` moved to `nest-rs-http`'s root, its
  third user, with `heard_at` for a peer heard on another task.
- **Written on the stream, never through rmcp's worker.** Sent through the worker,
  a ping and its answer would re-arm its idle timer, so an idle client answering
  would never idle out, and with no stream open the ping would wait in the
  session's cache and be replayed to the next one. The stream wrapper writes it
  ahead of what the stream holds, its answer stops in the session manager
  (`is_ping_answer`), and any message the manager admits for the session — a
  `GET` included — is the client heard.
- **Only a session holding a stream is asked.** One holding none has no channel,
  and holds no connection a vanished client would leave open; its idle timeout
  bounds it while the server writes nothing to it. Several streams of one session
  each ask; the first to give up says it, once.
- **The session ends by its worker's token**, which rmcp's own end then follows:
  the place frees, every stream of it ends, the next request reads `404` — or,
  with a session store, restores the session, whose record only a client's
  `DELETE` removes.
- The ids are strings, `ping-` and a random UUID: rmcp numbers its own requests,
  so no request of rmcp's, or of a host writing through its peer, takes one, and
  the specification's rule that an id is never reused in a session holds across a
  restore from the store and another instance serving the session, each of which
  gets a client of its own with no count to go on. Refused: `ping-<n>` numbered
  per client, which a restored session numbered from 1 again.

**Refused:**

- **`Peer::send_request` with a timeout** (rmcp's own request path): through the
  worker, with both faults above, and its timeout ends the request, not the
  session.
- **A per-session task** to time the wait whether or not the stream is polled:
  the transport polls a stream whenever it can write, and a stream it cannot write
  to is the kernel's to end; a task per session pays for a case the kernel bounds.
- **`TCP_USER_TIMEOUT`**: Linux's alone, and behind a proxy the server's hop is the
  live proxy.

Left open, in `TODO.md`: a stream the transport cannot write to (the kernel's
buffer full toward a vanished client) asks nothing until the kernel gives up; a
session whose stream is gone while the server keeps writing to it never idles out.
