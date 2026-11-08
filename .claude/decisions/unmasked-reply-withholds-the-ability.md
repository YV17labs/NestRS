# A route no mask shapes withholds the ability its guard attached

Until 2026-10-12 the ability reached a route's reads only where the route's
response mask armed it: `AbilityGuard` put it on the request, and the route
shaper installed it around the handler when, and only when, a mask would shape
the reply. A read on an unmasked route therefore found no ability and failed
closed, instead of answering rows the mask would have narrowed.

With one guard contract, what a guard attaches is ambient for the rest of the
unit once its stage passes, the ability included, so the shaper no longer
decides whether it is installed. **The shaper attaches `UnmaskedReply` on a
route no mask shapes, and `current_ability` answers `None` under it**: the reads
fail closed exactly where they did. The marker is core vocabulary because the
HTTP edge, which knows whether a mask runs, knows nothing of authorization, and
authorization, which owns the ability, cannot see the route's mask. It is
attached only where there is something to withhold — the stage attached a
value, or the unit carried one in — so a public route pays nothing for it.

**An `#[sse]` stream is such a route.** It takes no shaper parameter, so no
mask shapes what it emits, and a read in the stream fails closed as a read in
its handler does. The ambient-context change expected an `#[sse]` route behind
guards alone to see the caller's ability; security decides the other way: a
stream's events are built from what it reads, with no mask between those rows
and the client, which is exactly the reply this marker exists for. A stream
that must read scoped rows waits for a mask the stream can arm.

**Refused:**

- **`AbilityGuard` attaching only on masked routes**: the guard runs before the
  shaper and reads the site's metadata, not its mask, which the route's
  posture arms inside the shaper; teaching it would copy the shaper's decision
  into authz.
- **The shaper taking the ability back out of the ambient context**: what a
  stage attached is installed for the rest of the unit and no later layer
  removes it, which is what lets every layer below trust what it reads.

## 2026-11-05 — every edge withholds it, and a visitor reads only where it named the read

Decided by the first value under the owner's words of 2026-10-12 ("privilégier la
sécurité … production doit être sécurisé, je te laisse arbitrer") and the owner's
"Tout faire (Recommandé)" of 2026-10-31 to the item making `#[public]` one word on
every edge. The fix stage of that item named its closing as the end of GraphQL's
public reads; `#[public(Action, Subject)]` keeps them
(`route-posture-declared.md`, this date).

A `#[public]` GraphQL operation, WS message or MCP operation read under the visitor's
row rules and answered every exposed field, since only HTTP attached this marker: a
grant reading narrower than what it served. **A unit no mask shapes withholds the
ability on every edge, whatever its posture says**: a bare `#[public]`, a covered unit
stating none, a `#[subject]`'s. `unmasked` keeps it: there the body masks, written on
the line. It is attached only where there is something to withhold: a site with no
guard and none at its request keeps no stage, so "a public route pays nothing" holds,
and a reader finds it in one function, `withhold_from_reads`.

The marker reaches one unit, and a GraphQL field below an operation is none: it runs
after that operation's future returned, and a relation's batch runs off it with what
the request carries. **So a visitor's ability is read only inside a unit that named
its read.** `class_gate` on an entity licenses the visitor for the unit
(`VisitorReadNamed`), and `current_ability` answers none for a visitor anywhere else,
at no cost to any other caller. A visitor's nested read answers nothing, with the same
`warn`. **Withholding stops at a job**: `Repo` reads no ability in a job as system work,
every row, so a job narrowed to a visitor by `with_ability`, and a task off a withholding
unit opening a job, read under what they hold. The harness's `TestApp::unit` licenses what
it installs, standing in for a unit naming its read.

**Refused:**

- **Ending GraphQL's public reads**, the closing that item named: a blog's feed is the
  feature's reason, and the line `#[public(Read, Entity)]` keeps it masked.
- **Tracking the root field's posture to withhold a nested field's ability from a
  caller a token backs**: a per-field lookup on the hot path, for a caller who reads the
  same nested rows under an `#[authorize]` root. A nested read runs no mask for any
  authenticated caller; that limit is named where it stands, and closing it is a mask at
  the relation's loader and at a field resolver's answer.
- **A schema extension around each root field carrying the marker into its children**:
  an extension turns every field of every schema onto async-graphql's slow path.

## 2026-11-05 — a nested read runs a mask for an authenticated caller

The second refusal above says a nested read runs no mask for any authenticated caller.
That was never so of a relation, whose batch masked, and answered `INTERNAL` under a
narrow grant; and it is closed for a `#[field_resolver]` answering a wire type, both by
the entry of this date in `mask-provenance.md`. A visitor's nested read still answers
nothing, for the reason above.

## 2026-11-06 — a connection hook is such a unit

The owner's mandate of 2026-11-05 ("on ne veut pas d'angle mort"). Run first: `current_ability()`
in `#[on_connect]` and `#[on_disconnect]` answered the ability the upgrade attached, so a hook
read scoped rows with no mask between them and `client.emit`, and the rule's "every edge" named
only a hand-written `ServerHandler` as an exception. **A hook withholds it too**, attached around
the hook's body and nothing else, so the socket's messages keep it. No caller reads there: the
demo's hooks count connections, and the live suite's write through the executor.

The hook attaches the marker whatever it carries, not through `withhold_from_reads`: it has no
stage to read, and it runs once per connection, where the marker is nothing beside the socket.

**The consequence named:** a by-id write through `Repo` in a hook is barred with its read, as on
any unit no mask shapes, and `masked_reply` there answers `NoAmbientAbility`. A hook's system
write is `Repo::insert_unscoped`, a visible line; a hook wanting to read under the caller's rules
is a key to add later, with no break, once a caller asks.

## 2026-11-08 — no reply mask is left, and the marker names a unit no posture opens

The wire row is judged where it is built (`mask-provenance.md`, 2026-11-08), so no edge
reads a reply back and nothing is left for `unmasked` to switch off: the key is refused
on every edge, naming why, and `masked_reply` is gone. What this decision withholds does
not move: a unit whose posture names no entity's read — a bare `#[public]`, a covered unit
stating none, a `#[subject]`'s, a stream, a connection hook — still reads under no ability,
since nothing named its read, and the marker keeps its name, `UnmaskedReply`, an internal
seam no app writes. What changes is only why a posture on an entity opens its unit's reads:
each row the unit answers is judged for its caller where it is built, whatever shape
carries it. A hook writing through `Repo` stays barred, and a hand-written `ServerHandler`
answers wire rows its body builds, judged as every other.
