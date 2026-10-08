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
