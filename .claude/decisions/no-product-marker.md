# A product module carries no marker to stand apart from the framework

A product module is named from its path, even when the framework exports a type
of the same name (`features/authn/module.rs` is `AuthnModule`); where both are
needed in one file, the framework's is written by its full path.

A prefixed variant, `app_authn` / `AppAuthnModule`, was adopted and removed. It
triggered on the namespace the umbrella re-exports rather than on an actual
ident collision, so fourteen of seventeen marked types carried a marker that
distinguished them from nothing — `nest-rs-authz` exports no `AuthzModule`, and
`nest-rs-oauth-server` exports no module at all. A marker right for three names
and noise for fourteen is not a convention, and `App` is no subject: unlike a
driver's `Redis`, it says nothing about where a log line came from.

Aliasing (`use … as`) is no alternative either: `#[module]` names a module in
boot errors by the struct's own ident at its definition, which an alias does
not change.
