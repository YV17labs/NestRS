# object_store

## A retried request does not say it is one

- **Seen** — 0.14.2: the retry loop sends a failed request again through the
  `HttpService` a connector built, as a request of its own; nothing it hands
  the service says which attempt it is.
- **Effect** — the HTTP conventions' `http.request.resend_count` cannot be read
  off the request a client sends.
- **Our side** — storage infers it: the witnessed unit keeps its last failed
  sending, its method and address hashed and never kept, and the next request
  alike is its retry (`crates/nest-rs-storage/src/store.rs`, `resends_of`). Two
  requests alike that the app itself sends after a failure read as a retry.
- **Canary** — none: the inference holds whatever object_store hands over.
- **Upstream** — not filed.
- **When fixed** — read the attempt off the request and drop the inference and
  its task-local. An hour.
- **Ticket** —

  > **Expose a request's retry attempt to an `HttpService`**
  >
  > `RetryableRequest` sends each attempt through the `HttpService` a connector
  > built, but an attempt cannot tell it is a retry, nor which one. An
  > application recording its client requests as OpenTelemetry's HTTP
  > conventions ask (`http.request.resend_count`) has to guess by comparing
  > requests. Could the retry loop put the attempt number in the request's
  > extensions (or a header the service may strip) before each send?
