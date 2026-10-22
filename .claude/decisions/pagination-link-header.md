# The next page is a `Link`, as RFC 8288 writes it

Settled on 2026-10-08, under the owner's mandate to decide the OpenAPI audit's
open questions by the project's values. A `#[crud]` list named its next cursor
in `x-next-cursor`: an `X-` field RFC 6648 retired in 2012, which no client
library recognises. Standards decided: the list answers
`Link: </posts?first=20&after=<cursor>>; rel="next"` (RFC 8288, and the IANA
`next` relation), as GitHub's and GitLab's APIs do. The target is a relative
reference built on the path the caller addressed, so a global prefix and a
version survive and the client-controlled `Host` is never named. The last page
carries none.

**Refused:**

- **An envelope body** (`{ items, next_cursor }`): the body stays a flat array
  because response masking works row by row on the exposed type
  (`data-layer.md`, *Response masking*); an envelope would make every edge's
  mask unwrap it.
- **Keeping `x-next-cursor` beside `Link`**: two names for one fact, and the
  break belongs in the major that makes it (7.0).

Reopened when a list carries a filter the link must also carry — then the link
keeps the caller's query whole.

## 2026-10-22 — a cursor no page issued is refused, and its text stays the key's

`after` was read with `Uuid::parse_str(..).ok()` at all three edges reading it
(the HTTP and GraphQL `#[crud]` lists, an auto-resolved relation), so a
truncated or mangled cursor answered `200` with the first page, and a sync loop
following `next` re-read the whole collection. The cursor is now a type,
`PageCursor`, parsed at the edge; one no page issued is the edge's client error
— HTTP's `400` through its `pipe_error`, GraphQL's `INVALID_ARGUMENT` field
error, the one a malformed id answers there — and an empty `after` is the first
page, so a client templating `?after=` on its first call still works. The refusal names the parameter, never the value
(`payload-never-quoted.md`).

**Refused:** an opaque, base64url cursor. The GraphQL list's protocol is "pass
back the last `id`" — it returns a plain list, no cursor to pass back — so an
encoded cursor would force a Relay connection on every `#[crud]` list, a schema
change for every client, and the masking cost an envelope body was refused for
above. The type gives the Rust side room: a later key type's cursor is that
key's own text, added beside `From<Uuid>`, and a keyset order other than the
primary key reopens this.
