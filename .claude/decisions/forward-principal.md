# A request-scoped forwarder is gated by the guard that produces its value

`forward_principal!` takes the principal type alone, with no module marker.
Discovery that mounts or exposes something — a route, a resolver, a tool, a
loader — is module-gated so a linked crate cannot serve through an app that
never imported it. A request-scoped forwarder mounts and exposes nothing: it
copies a value only if something already put that value on the request. Its gate
is therefore the module-gated guard that produces the value, and a second gate
below it would cost an empty marker struct in every consumer.

It reads as a relaxation and is not one: the condition that activates the
forwarder — the value is present — is strictly narrower than a provider
registration, since a registered marker would fire the forwarder whether or not
anyone authenticated.
