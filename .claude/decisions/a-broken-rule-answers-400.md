# A value breaking its type's rules answers `400`, whoever checks it

Since 7.0 the edge holds every decoded value to the rules its type declares,
and refuses one that breaks them with `400 validation failed`
(`nest_rs_http::pipe_error`). A service that ran `validate()` itself returned
`ServiceError::Validation`, which answered `422`: one mistake met two
statuses, depending on who checked. Both carry `INVALID_ARGUMENT`.

## 2026-10-31 — one status for a broken rule

- **Decided**: `400`, on the edge and in `ServiceError::Validation` alike.
  RFC 9110 §15.5.1 makes `400` the client error a server will not process;
  §15.5.21's `422` is narrower, the request *content* well formed but its
  instructions unprocessable. A rule a type declares binds a path parameter,
  a query and a header as much as a body, and a `422` for a query string
  stretches "content"; `400` reads true at every location. The OpenAPI
  document already advertises `400` for every route an argument can refuse,
  and NestJS's `ValidationPipe`, the bar for this layer, answers `400`.
- **`422` stays `ServiceError::Invalid`'s**: a value that passes its rules
  and the service refuses on business grounds (a balance, a state) — the
  instructions §15.5.21 names, which only the service can judge.
- **Refused: `422` for every broken rule**, the edge's included. It is the
  more specific status for a body, but it would put two statuses on one
  route by location (a body's rule `422`, a query's `400`) or misname a query,
  and every document the framework writes would have to learn to tell a type
  declaring rules from one declaring none to advertise it.
