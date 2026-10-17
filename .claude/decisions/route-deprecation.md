# A route is deprecated with `#[api(deprecated = "YYYY-MM-DD")]`, and says since when

Settled on 2026-10-08, under the owner's mandate to decide the OpenAPI audit's
open questions by the project's values. OWASP API9:2023 asks for "a retirement
plan for each API version", and RFC 9745 (March 2025) gives a client the
`Deprecation` header to read it.

`#[api(deprecated = "2026-10-08")]` on a handler marks the operation
`deprecated: true` in the document, and every answer of the route, a guard's
refusal included, carries `Deprecation: @<the date's Unix time>`. The value is
an RFC 3339 full-date because the header is a date; anything else is a compile
error.

**Refused:**

- **Reading Rust's own `#[deprecated(since = …)]`**, tried first. It deprecates
  the method for its Rust callers, and a handler's only caller is the
  framework's wrapper, which then had to silence the lint; clippy's
  `deprecated_semver`, deny by default, refuses the date `since` would need,
  since it holds `since` to a crate version. Two meanings in one attribute, so
  `#[routes]` and `#[controller]` refuse `#[deprecated]` and name the key —
  one way to deprecate a route.
- **A `Sunset` header** (RFC 8594, Informational): a key carrying a removal
  date is a feature no route has asked for.
- **A `Deprecation` header on a handler's `Err`**: it is rendered past the
  route, where the route's wrapper no longer holds the response.

Reopened when a deployment needs a sunset date per route.
