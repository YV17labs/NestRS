# An MCP session is bound to the principal that opened it

## 2026-10-22 — bound at `initialize`, answered as unknown to anyone else

A legacy MCP client (before `2026-07-28`) names its session on every request
with `Mcp-Session-Id`. Every request is authenticated by its own bearer, as the
specification requires ("MUST NOT use sessions for authentication"), but
nothing tied a session to who opened it: a caller holding a valid token of its
own and another principal's session id could post into that session, open or
resume its stream and read what the server sent its owner, or close it. MCP's
security best practices (2025-06-18, *Session Hijacking*) answer it: "MCP
servers SHOULD bind session IDs to user-specific information … such as their
internal user ID".

**What a session is bound to**: the actor id and the tenant the authentication
guard read off the verified principal, captured at the endpoint after the
request's guards. Neither is a `Claims` type the framework would have to name.
The actor alone would let two tenants' principals sharing an id, or two
machine principals with no actor id, share sessions; the tenant is already the
framework's authorization-grade identity of a principal. This is not the
"`actor_id` is audit, never authorization" case: at the endpoint the actor is
written only by this request's authentication guard (write-once, never read off
the wire, unlike a job's, which its envelope carries), and the binding grants
nothing — the guards still decide what each request may do. It keys the
session the request may address, as rmcp's own lookup keys it by id.

**Equality, both ways.** An anonymous session binds the anonymous principal: a
caller presenting a credential does not reach it, nor an anonymous caller an
authenticated principal's. A client whose credential changes identity
mid-session (a login on a public mount) is answered as for a session it does
not have and opens a new one, as the transport specification tells it to. A
public endpoint (`McpOptions::posture = Public`) runs no guard at its request,
so every caller is anonymous there and the session id is a session's only
key — there is no principal to bind to.

**The answer is the unknown session's**, `404` with rmcp's own `Not Found:
Session not found`, byte for byte, so the status never says whether another
principal's session exists; one `warn` on `nest_rs::mcp` names the call that
refused it, never the session or its owner. rmcp answers a `DELETE` of a
session it does not hold `202`, which would have said exactly that; the
transport specification answers a terminated session `404`, so the endpoint
does, for both.

**Checked at every call that reaches a session**, not once in front of rmcp:
rmcp lets a request waiting on another request's restore of the same session
past its `has_session`, so the endpoint installs who the request is served for
on the task answering it and each session-manager call compares it.

**Refused before rmcp registers a restore.** rmcp 3.5 restores one session for
one request at a time: the first request naming a session it does not hold
registers a restore, and every later one waits on it and takes its outcome. A
refusal inside the store's `load` came after that registration, so for a
whole store round trip another principal's request held the owner's own
requests on its failure, on every instance not holding the session in memory
— a hijacker could end its owner's session at will. `has_session` therefore
reads the stored owner itself when the instance does not hold the session, and
refuses another principal's with an error, which rmcp answers at once without
registering anything (the endpoint answers it `404`); a `false` would still
register a restore that fails. What it read is kept on the request for the
`load` that follows, so a restore costs one store read and a session stored
without its principal is said once.

**The binding travels with the session.** rmcp's stored state is its
`initialize` params alone (`SessionState` is non-exhaustive, one field), so the
mount writes the principal into the params' `_meta` under `dev.nestrs/principal`
— `_meta` keys take a reverse-DNS prefix — overwriting anything the client put
there, and takes it out before rmcp replays the handshake. A state stored
without it, by a release before the binding, is refused to every caller rather
than adopted by the first to name it; its client re-initializes.

Refused: binding to the bearer itself (a refreshed token would lose the
session); a signed session id (a key to share across instances for what the
store already carries); checking only in front of rmcp (the restore race
above).

Left as rmcp's: a `tools/call` naming a tool no host declares is answered
before rmcp whatever session it names, reaching no session and reading the same
for every session id (`endpoint.rs`, `UndeclaredTools`); and rmcp 3.5 removes a
failed restore's entry on a task of its own (`PendingRestoreGuard`), so a
request restoring the same session in that instant reads the failure as its
own. Another principal's request registers no restore of a session the
instance does not hold, so that instant follows only a restore failing for
every caller (a session the store does not hold, or holds without its
principal) or the owner's own (the cap, the store failing).

## 2026-10-22 — a principal naming no subject opens no session

**Correction.** The entry above says the tenant keeps "two machine principals
with no actor id" apart. It does so across tenants only: two such principals
in one tenant, or in none, were one principal, each served the other's
sessions — a `POST`, the standalone stream and its resume, a `DELETE` — and
with no tenant such a principal was also the anonymous one. A tenant is not
"user-specific information" in the sense of MCP's security best practices
(2025-06-18, *Session Hijacking*): every machine client acting within it
shares it.

**What a session is bound to, on a gated mount, is the subject** — the
`actor_id` the authentication guard read off the verified principal, the
tenant still beside it, so one id in two tenants stays two principals. A
principal the guards admitted with no subject has nothing of its own to bind
a session to, so none opens for it: its `initialize` is answered `403`, a
problem document whose code is `FORBIDDEN` and whose detail says why, with no
session id, and one `warn` on `nest_rs::mcp` (`mcp session refused: its
principal names no subject`, a `hint` beside it) names neither the credential
nor the tenant. The session manager refuses it in `create_session`, the one
call that opens a session; the cap's check in front of rmcp answers it the
same way when the mount is full, never with the cap's `503` and its
`Retry-After`, since no freed place would change the answer. A session the
release before stored for such a principal is adopted by no caller on a gated
mount, said at `warn` as stored for a principal naming no subject. A public
mount is unchanged: no guard runs there, every caller is anonymous, and the
session id is a session's only key.

The posture is the app's declared one (`McpOptions::posture`), read once at
mount. A hand-built endpoint (`McpMount::new`, the test seam) is gated, as an
app's endpoint is until the app opens it; `McpMount::public` opens it.

**Why refuse rather than bind to something else**: no standard the framework
verifies binds such a principal uniquely. RFC 9068 §2.2 makes `sub` REQUIRED
in a JWT access token — the client's own id when it acts on its own behalf —
so a conformant credential always names one, and the authentication page
tells an app to return it from `actor_id`. Binding to the credential itself
stays refused (above), and the endpoint cannot tell which header a guard
read. A sender-constrained credential (RFC 8705's certificate thumbprint,
RFC 9449's DPoP key) would bind uniquely, and the framework verifies neither.
A client with no subject still calls statelessly, on protocol `2026-07-28`,
which opens no session; so does every client once
`<PREFIX>_MCP__LEGACY_SESSION_MODE` is off.

**Tasks, the family's other state addressed by id.** rmcp holds no
`TaskManager`: a host does, and the endpoint builds its hosts for each
session (and for each stateless request), a field that is not `#[inject]`ed
taking its `Default`. A task therefore lives in the session that started it,
reached only through that session, which serves its principal alone (held by
`a_task_is_reached_only_through_the_session_that_started_it`). A
`TaskManager` an app shares across sessions through a provider it injects is
the app's own state, and its `tasks/*` methods decide who may reach each
task: the framework sees no task created, so it has nothing to bind. Refused:
binding task ids at the composite handler, which would read every
`CreateTaskResult`, keep a map under a retention it does not own, and still
miss a host answering task ids its own way.
