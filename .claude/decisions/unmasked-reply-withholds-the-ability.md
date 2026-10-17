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
