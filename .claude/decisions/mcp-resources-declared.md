# An MCP resource is declared like a tool

A resource is a method of the `#[tools]` block, with the posture, guards,
filters, interceptors and mask a tool takes. `#[resource(template = "…")]`
reads the URIs an RFC 6570 template writes, each of its variables a parameter
of the method; `#[resources]` is the method answering `resources/list`.
`#[tools]` writes `list_resources`, `list_resource_templates` and
`read_resource` itself, rmcp having no router for them, and derives the
`resources` capability from the two roles. Decided by the owner on 2026-10-28
("Tout faire", the identity audit's feat-3).

Until then resources had no decorator, on the reason that they were "a mapping
from URIs to rows rather than a set of methods". A template read is a method
with arguments, and the cost of the raw shape fell elsewhere: a host serving a
resource wrote its whole `ServerHandler`, so its tools and prompts went under
rmcp's `#[tool_router]`, where the same `#[tool(description = …)]` compiles
with no posture — no class gate, no mask, no layer — and it restated by hand
the capabilities, the URI parsing and the refused-row answer the decorator
derives. The demo's posts host was that shape, its `list_posts` and
`draft_follow_up` ungated.

- **A template is read back, so it is level 1.** A variable is `{name}`, named
  like the parameter that takes it; operators and modifiers (`{+path}`,
  `{/a*}`, `{id:3}`) and two expressions with no literal between them are
  refused at the template, since a URI does not decompose back into them by
  name. A value is what simple expansion writes (RFC 6570 §3.2.2): unreserved
  characters and percent-encoded octets, one at least, percent-decoded and then
  parsed with `FromStr`, ending at the first occurrence of the literal after
  it. The decorator splits the template once; the runtime only matches.
- **A URI is offered to the templates in declaration order**, and the first
  whose variables all parse reads it. Two equal templates in a host do not
  compile; two hosts on one path reading one template fail the boot, as a
  claimed tool name does.
- **One answer for what cannot be read.** A URI no template writes, a variable
  that does not parse, and `None` from the method (`Option<B>` is a resource
  the caller may not read) are MCP's *Resource not found* — rmcp turns
  `-32002` into `-32602` from `2026-07-28` — under the `NOT_FOUND` class, never
  quoting the URI, so a reader cannot tell a row it was refused from a row
  that does not exist.
- **The answer is read by its type**: `Json<T>` as `application/json`, a
  `String` as `text/plain`, a `ReadResourceResult` as built; the template
  advertises the media type through a probe, so a value that is no body is
  refused once, at the read.
- **The mask is the tool's, where it can read.** It unwraps `Json<T>` and a
  resource's `Option<Json<T>>`, and refuses a masked `ReadResourceResult` as it
  refuses a `CallToolResult`. A listing's entries are URIs and names the
  round-trip cannot read, so a gated `#[resources]` writes `unmasked`.
- **A site names its template, never its values.** The view a guard reads and
  the address a denial files are the template; the URI the client sent is the
  request's, on its span as the conventions ask.
- **One listing per host**, no argument but `Scoped<T>`: `resources/list` sends
  none, and a second listing would never run. A listing beside no
  `#[resource]` is refused: it would name URIs nothing reads.

**Refused:**

- **Levels 2–4 of RFC 6570**: they expand to text that reads back ambiguously
  (`{+path}` keeps reserved characters, an exploded list has no arity), and no
  caller needs one.
- **`bind = Service` on a resource**: the template's variable could be the
  binding's id, but the body's `access` is three lines and the binding would be
  a second loader beside HTTP's and GraphQL's for one caller.
- **A pagination cursor on the listing**: one caller lists every row today;
  `Vec<Resource>` is the shape until one needs a page.
- **Masking a listing silently as a no-op**: a gate with no mask must be
  written, `unmasked`, where a reader sees it.

The hand-written `ServerHandler` stays for what no decorator declares —
completion, logging levels, subscriptions, the `tasks/*` trio, custom methods —
and completion is its witness, in `nest-rs-mcp`'s registry suite.
