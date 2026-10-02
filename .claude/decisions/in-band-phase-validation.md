# The in-band guard chains are not phase-validated

`boot_validate_guards` turns a misordered chain — an authorization-phase guard
ahead of the authentication-phase one whose principal it reads — into a named
boot failure instead of a deployment that denies everything with nothing to say
why. It runs at HTTP's route mounts, over the global bucket, and at a WS
gateway's upgrade (an HTTP `GET`, so it runs `check_http`, the entry the check is
written about).

Per WS message it was shipped and proved wrong in both directions at once. A
per-message chain runs `check_ws_message`, while the validation reads what a
guard produces and expects for `check_http` — and `AuthnGuard` keeps the no-op
`check_ws_message` by design. Applied there it was silently green on a chain
where nothing attaches a principal, and a false boot failure on the split-scope
shape the authorization docs sanction. The ordering half would transfer; the
principal half needs a per-message notion of "produces".

`#[operations]` and `#[tools]` compose their chains on the first dispatch that
reaches a site, after every boot check, and nothing at boot enumerates the
sites. Validating them would need a link-time registry of sites walked at boot.
