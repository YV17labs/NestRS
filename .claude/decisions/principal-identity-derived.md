# A claims type derives what the framework reads from it

## 2026-10-30 — `#[derive(PrincipalIdentity)]`, its roles read by type

Every app wrote the same two impls on its claims type: an empty
`impl AmbientValue`, so the authentication guard could attach it, and an
`impl PrincipalIdentity` whose methods each returned one field. The identity
audit's agreement ("Tout faire", 2026-10-28) covered a derive for them; it
needed a crate of its own, a proc-macro crate exporting only macros, so it
ships in `nest-rs-authn-macros`, re-exported as `nest_rs::authn::PrincipalIdentity`
beside the trait — the `<crate>-macros` set, as `#[subject]` ships in
`nest-rs-authz-macros`.

**A field attribute names the role it plays**: `#[principal(actor)]`,
required, and `#[principal(tenant)]` and `#[principal(scopes)]`, each played by
one field, one field free to play several. A role left unmarked answers the
trait's default. The keys go through `nest_rs_codegen::Grammar`, so a role
marked twice or one the derive does not know is refused in the shared
sentences.

**An actor or a tenant is read by its type.** The trait returns
`Option<String>`, and a claim the credential may leave out is an `Option`, so
the derive asks an autoref probe in `nest_rs_authn::__private` per field: an
`Option<T: Display>` answers its value or `None`, any other `Display` type the
text it prints. A type alias of `Option` is an `Option`.

**The scopes are a list.** `scopes` answers `Some` of the marked field, which
coerces to `&[String]`; the token's `scope` claim deserializes into it, empty
when absent, so a credential granted nothing is delegated nothing.

**The derive writes `AmbientValue` too, with its default carry.** A principal
must be ambient (`Strategy::Principal: AmbientValue`), so every claims struct
the derive accepts can be; the shapes that cannot be are refused rather than
given a partial impl.

**Refused:**

- **`Option<Vec<String>>` for the scopes**: `None` reads as a credential scope
  is no dimension of, to which every scoped rule applies in full, so a token
  sent without its `scope` claim would gain what one granted nothing is
  withheld. Fail-closed is the derive's; a principal that means `None` writes
  the impl by hand, a visible line.
- **Reading `Option` off the field's spelling**: a renamed or aliased `Option`
  would read as a value and refuse to compile, against the macros rule that a
  decorator reads by type.
- **Two probe traits**, one per autoref depth: each expansion would import both
  and leave one unused. One trait with two impls, on the probe and on a
  reference to it, resolves at the same depths.
- **A generic claims struct**: a lifetime cannot be `'static`, which an ambient
  value is, and a type parameter would need bounds the derive cannot choose.
  Such a principal writes both impls by hand.
- **An opt-out of the `AmbientValue` impl**, or a key narrowing its carry: no
  principal the framework, the demo or the docs know narrows it — a feature
  for a hypothetical user. One that must, writes both impls.
- **A struct-level attribute naming the fields** (`#[principal(actor = sub)]`):
  the role sits on the field it reads, where a rename carries it.
