# A payload value is described, never quoted

`CLAUDE.md`'s hard "no" forbids a payload value or a secret in an error, a log
line, a stored record or a reply; a decode failure says where, what kind of
value and what the type expected. The reason is that every value the framework
decodes is somebody's data: a client's body, a producer's job, a deployment's
config. Quoting it moves that data into a log pipeline, a dead-letter record or
a `400` body, and a `400` is kept by proxies and caches the framework never
sees. A description carries everything a developer needs to fix the call.
