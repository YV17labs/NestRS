# The documentation is a development surface, opened in production by one visible line

Settled on 2026-10-08, under the owner's mandate to decide the OpenAPI audit's
open questions by the project's values. OWASP API9:2023 asks that "API
documentation [be] available only to those authorized to use the API", and
ASVS 5.0 13.4.5 that documentation endpoints are "not exposed unless explicitly
intended".

`OpenApiModule` serves `/api` and `/api-json` by default in the development and
test profiles only. Outside them a deployment opens them with
`<PREFIX>_OPENAPI__ENABLED=true`, and the boot logs that the endpoints are public
at `warn`. Security decided: safe by default, and the opening is one visible
line.

**Refused:**

- **The documentation behind the app's own guard chain.** The API's guards read
  a bearer token, which a browser navigating to `/api` cannot send, so a
  guarded page could never load the UI that asks for the token. A deployment
  that publishes its documentation to a restricted audience gates it where
  browsers authenticate: its proxy or its SSO.
- **Filtering the document per caller** (OpenAPI 3.2 §6.4 allows it): a
  document that differs by caller cannot be committed, diffed or generated
  from, which is what it is for.

Reopened when the framework gains a browser session a guard can read.
