# authn and authz are two crates, not two crate families

A shared crate prefix is a family only when one external standard names each
member (`architecture.md`, *Families*). Grouping the auth territory as
`authn-*` / `authz-*` was considered and retired on that test.

The instinct is sound — authentication and authorization are the two halves of
the territory — but the membership cannot be tested. RFC 6749 titles itself
*The OAuth 2.0 Authorization Framework*, so a client and an authorization
server are authz; social login is authentication performed through that
authorization flow; a JWT verifier serves both, because one token carries the
identity and the scopes; and RFC 9728 discovery is served to callers with no
identity at all, so it is neither. One candidate in five classified without
argument. `authn` and `authz` stay as crate names, because the pair is the one a
reader wants; the OAuth roles became the `oauth` family, read off RFC 6749 §1.1.
