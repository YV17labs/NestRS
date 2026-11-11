# The OpenAPI document is 3.1, and states only what the framework knows

Settled on 2026-10-08, when the owner asked that every question the OpenAPI
audit left open be decided by `CLAUDE.md`'s values rather than brought back.
The audit ran the demo's document through Redocly CLI 2.60, Hey API 0.99 with
its TanStack Query plugin and openapi-typescript 7.13, and compared the
generator against Huma, FastAPI, ASP.NET Core 10, utoipa 6, NestJS and
springdoc.

**The document is OpenAPI 3.1.2**, the 3.1 line's current patch, while 3.2.1
(2026-09-10) is the latest release. A 3.1 document is valid 3.2 once its
version string moves, but the client generators a frontend uses read a 3.2
document as 3.1 and drop, without an error, what only 3.2 says: Hey API and
openapi-typescript have no `query` method and type an `itemSchema` stream
`unknown` (hey-api/openapi-ts#2660, openapi-ts#2577), Kubb has no `QUERY`, and
openapi-generator fails on any 3.2 document (#24212). Correctness decided:
a contract its readers silently truncate is not the contract. utoipa 6 made
3.2 opt-in for the same reason.

**Refused:**

- **A 3.2 opt-in, and typed event streams with it.** An `#[sse]` route's events
  are `string` because `SseStream` is untyped and 3.1 has no item schema; 3.2's
  `itemSchema` is the standard place, and the generators do not read it yet.
  Writing `itemSchema` into a 3.1 document, as FastAPI 0.143 does, breaks
  orval's validation (orval#3072). No feature for a reader that does not exist.
- **An `oauth2` or `openIdConnect` scheme for an OAuth resource server.**
  `bearerAuth` is exactly what the guards check. The authorization server's
  endpoints are not known to the resource server without fetching its metadata
  at boot, and no route states its scopes statically, so the scheme would be
  invented. 3.2's `oauth2MetadataUrl` (RFC 8414) is the standard form, and
  comes with 3.2.
- **A `discriminator` on tagged enums.** schemars writes a `oneOf` whose
  variants carry a `const` tag, which TypeScript narrows on its own; a
  discriminator is OpenAPI vocabulary that needs a `mapping` to be read right
  (openapi-typescript#2149), for no gain in the generated types.
- **A boot error for two operations publishing one `operationId`.** The routes
  serve correctly; only the document is affected, and the boot `warn` names
  both operations and the rename that ends it. Huma panics; a boot refusing
  working code escalates past the fact (`container.md`, *Discovery*).
- **The `RateLimit` and `RateLimit-Policy` headers.**
  draft-ietf-httpapi-ratelimit-headers-11 is a draft; a `429` states its wait
  in `Retry-After` (RFC 9110), and the framework writes the IETF fields once
  they are an RFC.
- **A `default` response.** It would type as a problem document a status a
  handler writes with a body of its own; each error status the framework
  answers is listed, and a handler declares its own with `#[api(error(…))]`.

Reopened when Hey API and openapi-typescript read 3.2 — then the document
moves to 3.2 whole, typed event streams and `oauth2MetadataUrl` with it — or
when an RFC fixes the rate-limit fields.

## 2026-11-05 — a guard names the scheme it challenges with

`bearerAuth` was written for every route whose guards can answer `401`, which was false of a
token endpoint's `Basic`. A guard now says its scheme (`Guard::denial_document`), defaulting to
`Bearer`, and the document defines and requires each scheme a route's chain challenges with.
This moves nothing the refusal above decides: `basicAuth` is exactly what that guard checks, as
`bearerAuth` is what the others do, and neither scheme is invented.

A chain challenging two schemes takes one requirement object naming both, never two entries:
OpenAPI 3.1's Security Requirement Object makes a `security` list's entries alternatives and an
object's schemes all required, and a chain runs every guard. A class gate's implied `401` joins
only a chain with no `401` of its own: it is reached past that one, and would offer `Bearer`
beside a guard challenging `Basic`, which is the symptom this entry closes.

## 2026-11-05 — a document says what a route answers, read live against it

Every status below was seen on a request and missing from the document (`nest-rs-openapi`'s suite
holds each against the live answer):

- **What the edge answers whatever the route declares is listed under the config it runs on**: the
  request budget's `503`, a body's `408` while the gap sits under the budget (at or past it the
  budget fires first, which the boot's `warn` says), and `403` under a CORS policy listing origins.
  A `Default` of none is a document no transport serves.
- **What a route's inputs can refuse is listed by the input**: `415` for a body read by its media
  type, never `RawBody` or `Body`, which read any bytes, nor a `DocumentedBody` refusing in its own
  words; `400` for any parameter, optional and path ones included, each able to carry a value its
  type does not read, a map's too, which names no property to list; `400` and `404` for a version
  parameter, a malformed token and a version nothing serves. `RawBody` and `Body` document `*/*`,
  optional and binary. A `Form<T>` on `GET` reads the query, so it is its `in: query` parameters
  and its `400`, none of a body's statuses.
- **A selector refuses in front of routing**: under the `Header` or `MediaType` strategy, once a
  route is versioned, every route lists the `400` of a malformed version, unversioned ones
  included. They list no `404`: a well-formed version moves no unversioned address. With nothing
  versioned the selector is not mounted, and none is listed.
- **A body sent to a route taking none is not its answer.** It is refused `413` past the cap, and
  no document lists that: the route's contract is not to be sent one.

**A status only the handler knows is `2XX`**, OpenAPI 3.1's range for it, with the body the answer
says: a `(StatusCode, B)` pair, a bare `StatusCode`, a built `Response`, an application's own
`IntoResponse`. A type states its status or the handler does, so the framework's own fixed answers
state theirs. The pair stays: it is a developer's common spelling, and removing it for one way to
say a status would make everyday code harder to fix a document (the owner's rule of 2026-11-05).
`#[http_code(N)]` pins it where the status never varies. `default` stays refused, for the reason
above: it would type as this body every status the document lists no other for.

**An answer documents the media type it writes**, read off its type by a probe of its own
(`DocumentedMedia`), so a `(StatusCode, String)` sits under `2XX` with `text/plain`, with no key for
the developer: text is `text/plain`, bytes `application/octet-stream`, a `Body`, which sets none,
`*/*`. It is no field of `AnswerDoc`: `Created<T>` states its `201` over any `T`, an application's
own answer included, and a const cannot read `T` only where it can, so a field would either leave
`Created<String>` out or send `Created<Own>` to the computed arm. A
`#[response_header("content-type", …)]` writes over what the type wrote, so it keys the body, and
`#[api(response_content_type)]` wins over both, the developer's own word.

**A guard declaring a status outside `400` to `599` fails the boot** (`HttpGuardStage::new`),
naming the guard, at every site: the document files a declared status over what else it says of
it, and a declared `200` stood there for the route's success.

**Refused:** removing `(StatusCode, B)` and bare `StatusCode` for `#[http_code]` and a built
`Response`; a new `#[api]` key for a computed status; `200` for what no type states, the sentence
the page used to carry about a built `Response`.

**Left, named to the owner:** a required `Ambient<P>` on a `#[public]` route answers a deferred `401`
the document does not list. Closing it takes the parameter's `P` read off the handler and matched
against the principal the chain's guard defers for, since an `Ambient<T>` a guard attaches defers
nothing.

## 2026-11-07 — every route lists the cap's `413`, a bare problem no success, and the connection's refusals no route

Each seen on a request, against `38bdfce04`'s document:

- **A route taking no body answers the cap's `413`**: `GET` and `DELETE` with a declared length
  past the cap were refused before routing, and the document listed no `413`. The entry above
  called that no answer of the route's; it is what the edge answers whatever the route declares,
  the rule that entry lists `503` by, so every operation lists it. A body with no declared length
  that no reader reads is never refused, so `408` stays where a reader waits.
- **A `ProblemDetails` answered as a handler's success answers no success**: `GET` answered `404`
  `application/problem+json` and the document said `2XX` with no body. It now lists no success and
  the problem under `4XX` and `5XX`, OpenAPI 3.1's ranges for an error the handler computes. An
  `HttpError` returned bare stays `2XX`: `from_response` may carry any answer.
- **What hyper answers before a request exists is no operation's**: a head that does not parse
  answers `400`, one past the 64 KiB buffer `431`, with no problem document and no layer's header,
  run over a socket. OpenAPI's responses are "the expected responses of an operation", and no
  operation received these, so none lists them; `openapi/index.mdx` says so.

## 2026-11-07 — a `#[public]` route needing its principal says its `401`, and answers it to every caller its guard did not identify

The limit named above, run on a `#[public]` route taking `Ambient<Caller>` behind `AuthnGuard`:
its document listed `200 500 503` and `security: []`, a rejected token answered `401`, and a caller
with none the wiring `500`, its `error` line saying the guard never ran. A later guard requiring the
principal on a `#[public]` route answered its `401` with the same document. Correctness decided:

- **`#[routes]` reads each `Ambient<T>` parameter's `T` off its spelling**, as it reads `Piped`, and
  the route's guard stage finds the guard of its chain producing it as its principal. On a
  `#[public]` route the stage refuses, once its guards passed, a caller that guard admitted without
  it: the `401` a rejected credential earned, else the caller's own, the plain challenge. It is
  the rule a later guard's `expected_principal` follows already, by cause, filed as that guard's
  denial, ahead of every argument a handler reads.
- **The document lists that `401`**, with the producing guard's scheme, whichever of the two needs
  the principal, so the route's `security` names the scheme where it said `[]`.
- **An `Ambient<T>` no guard of the chain produces as its principal defers nothing**: a context a
  guard attaches stays the wiring `500` when absent.

Refused: **refusing the shape at boot.** A guard attaching a visitor principal for every caller
makes `#[public]` beside a required `Ambient<P>` a route that serves everyone; the boot cannot tell
that guard from `AuthnGuard`. **Recording an anonymous caller on every `#[public]` request** so the
extractor answers it: an allocation on every public request behind an authentication guard, the
probes' included, for the routes that need it alone.

Left: an `Ambient` behind a type alias is not read off its spelling; such a route keeps the old
answers, the earned `401` and the anonymous `500`.

## 2026-11-11 — an `Ambient` behind an alias needs its principal as the name does

The limit left above, reproduced: a `#[public]` route taking `type Me = Ambient<Caller>` behind
`AuthnGuard` answered an anonymous caller `500` and its document listed `200 413 500 503`, no
`401`. `#[routes]` read the `T` of each parameter spelled `Ambient`; the alias was an extractor
like any other.

- **A route asks each parameter's type** what it reads off the ambient context, through the
  probe the access graph already asks for a `Scoped<T>` (`ParamProbe`, its `Ambient<T>` arm),
  so the principal the route needs, its `401` and its document hold under any name.
- **The edges carrying operations keep reading `Ambient<T>` and `Scoped<T>` by their names**: a
  GraphQL operation's, a WebSocket message's and an MCP operation's arguments are written into
  the protocol's own signature before any type is known, so which parameter is the protocol's
  is the macro's to decide. Every other parameter is asked whether it is one of the two, and an
  alias of either fails to compile naming the fix, beside the protocol's own refusal of it as an
  input, where rmcp's alone named `IntoToolRoute`.

Refused: **letting the edges carrying operations take an alias.** A parameter left out of the
protocol's arguments by its type would need one trait choosing per type between the protocol's
input and the unit's value: its impl for `Ambient<T>` overlaps a blanket over async-graphql's
`InputType` or serde's `Deserialize`, which coherence refuses, and it would hand a value to a
schema the macro has already written.
