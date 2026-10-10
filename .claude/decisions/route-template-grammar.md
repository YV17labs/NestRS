# A route template is OpenAPI path templating

A route path in `#[routes]` is read with poem's grammar — `:name`, a parameter
constrained by a regular expression (`:name<re>`, `<re>`), `*` and `*name` —
and the macro's refusals name poem ("is not a path poem can mount"). The
grammar is a public contract written in every controller, and it is poem's by
definition: the OpenAPI document rewrites `:id` as `{id}` and drops what it
cannot template, the throttler keys on poem's `PathPattern`, and an adapter
swap would break every app in a minor.

**The grammar is nestrs's, and it is OpenAPI's path templating** — the
spelling the document already emits. A segment is literal text, or `{name}`
taking the whole segment or following literal text inside it (`/@{handle}`),
with nothing after it in that segment; `{*name}` comes last, is named, and
never matches an empty rest, since the edge trims trailing slashes first;
`{{` and `}}` are literal braces. A template's identity drops its parameter
names, as today. One spelling serves the code, the document, the span, the
access line and the throttler's key.

**What the next router cannot route is refused at compile time**, whatever the
spelling: a parameter with a pattern (parse it with its type, `Path<u64>`, or a
pipe), text after a parameter in its segment (`/{id}.json`), an unnamed or
non-final catch-all. matchit 0.8 — axum's router, which a later adapter sits on
— refuses the same ("Prefixes after route parameters are not supported") and
accepts a literal prefix before a parameter. The 6.x `:id` and `*rest` are
refused with a sentence naming the 7.0 spelling, and the macro's sentences stop
naming poem: an address is mounted once, and each parameter binds by its name.

A template that exists only at boot (`also_mounts`, the GraphQL path read from
config, `HttpTransport::mount`) is read by a runtime twin of the macro's
parser; one corpus pins both. While poem routes, the transport translates each
template into poem's spelling at mount, and the template recorded for the
span, the access line and the throttler is the declared one.

**Refused:**

- **poem's grammar as the contract**: it binds every app to one router, and
  its regular-expression parameters and suffixes are what the next router
  does not serve.
- **Keeping `:id`**, NestJS's and Express's convention, as nestrs's own grammar
  translated per adapter: a second spelling beside the document's, the span's
  and the throttler key's, and a convention where a specification exists.
  Spring, ASP.NET Core, Go 1.22's `ServeMux`, axum 0.8 and matchit spell
  `{id}`. NestJS sets the bar for what nestrs offers, not for its syntax.
- **Regular-expression parameters**: a parameter is parsed by its type or a
  pipe, where a refusal is typed and named.

If the owner vetoes the spelling before it merges, only the parameter token
returns to `:name`; the refusals stay.

**Status (2026-10-10): decided, not landed.** It lands with
`refactor/route-grammar`, which appends an entry here where it departs.
