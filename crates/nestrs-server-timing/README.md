# nestrs-server-timing

W3C [Server-Timing] interceptor for nestrs. Importing `ServerTimingModule`
adds a `Server-Timing` header to every response so browsers render the cost
in their Network panel. Handlers record sub-step durations by pulling
`Timings` out of request extensions.

[Server-Timing]: https://www.w3.org/TR/server-timing/

## Extending

This crate is one `Interceptor` from `nestrs-middleware` — the seam is the
`Interceptor` trait, not anything in this crate. To ship an alternative
response-timing header (a different format, an additional `X-Response-Time`,
sampling) write your own `Interceptor` and import it instead of (or
alongside) `ServerTimingModule`.

To extend *within* the Server-Timing spec — add custom entries to the
generated header — handlers already do this through the `Timings` extension:

```rust
async fn handler(timings: nestrs_http::Ctx<Timings>) -> impl IntoResponse {
    timings.record("db.query", duration);
    // …
}
```

There is no community-crate naming convention here; the interceptor is
small and orthogonal — most replacements live next to the app that needs
them, not as a published crate.
