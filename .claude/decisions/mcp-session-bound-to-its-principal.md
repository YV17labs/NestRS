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
failed restore's entry on a task of its own, so a request restoring the same
session in that instant reads the failure as its own.
