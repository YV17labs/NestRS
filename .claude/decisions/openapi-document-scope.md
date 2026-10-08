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
