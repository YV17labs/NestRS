# nest-rs-authn

Authentication for nestrs — establishing *who* the caller is. Composable
machinery (`jwt`, `oauth`, `passport`, `password`); the product wiring
(`Claims`, the `AuthGuard` type alias, the OAuth controller) lives in
`crates/features`.

## Extending

The open seam is the [`Strategy`] trait — one request in, one principal
(or a challenge) out:

```rust
pub trait Strategy: Send + Sync + 'static {
    type Principal: Clone + Send + Sync + 'static;
    async fn authenticate(&self, req: &mut Request)
        -> Result<Outcome<Self::Principal>, AuthError>;
}

pub enum Outcome<P> {
    Authenticated(P),
    Challenge(Response),  // a 401 body, an OAuth redirect, …
}
```

[`AuthGuard<S>`] is generic over `S: Strategy`. Bind one with
`#[use_guards(AuthGuard<MyStrategy>)]` and the guard attaches the principal
to the request extensions; a handler reads it back with
`nest_rs_http::Ctx<MyPrincipal>`.

Built-in strategies:

- [`JwtStrategy<C>`] — bearer JWT, claims `C: DeserializeOwned + Clone`.
- The OAuth Authorization Code client (`OAuth2Client`) is *configuration*,
  not a `Strategy` — apps that need an OAuth strategy implement one in
  `features::oauth/strategies/` (the OAuth strategy needs DB-backed
  state which is feature-side concern).

A community impl is named `nest-rs-authn-<scheme>` — e.g.
`nest-rs-authn-opaque-tokens` (token introspection against a remote
authorization server), `nest-rs-authn-mtls` (client certificate),
`nest-rs-authn-passkey`. Each ships a `Strategy` implementor (an
`#[injectable]` struct) and, when it has runtime config, a `<Name>Module`
that registers the strategy as `Arc<Self>`.

[`Strategy`] is the *only* surface a new authentication scheme needs to
extend. `JwtService`, the OAuth client, password hashing, and the
challenge helpers are utilities a `Strategy` impl may use — not seams a
new scheme must replace.
