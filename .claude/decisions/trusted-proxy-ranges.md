# A trusted proxy is an address or a CIDR range, and a mapped address is the IPv4 it carries

Settled on 2026-11-05, on the owner's mandate of that day, from two runs against
`Vec<IpAddr>`: `10.0.0.0/8` failed the boot, and on a listener on `[::]` the entry
`127.0.0.1` never named the peer the socket reports as `::ffff:127.0.0.1`, so every
caller read as the proxy. Behind an ingress whose pods move no address list holds,
so every caller shared one throttler bucket and one `ClientIp`.

**`HttpConfig::trusted_proxies` is `Vec<TrustedProxy>`**, read from the same
variable, taking both forms. `TrustedProxy` has private fields, so what it refuses
it refuses from the environment and from code alike, and no check waits at boot.
Security decided the refusals, Elegance the shape:

- **`/0` is refused**, `0.0.0.0/0`, `::/0` and the mapped `::ffff:0:0/96` alike. With
  every address trusted, no hop is ever the client, and the degenerate answer is the
  outermost hop, the one a caller wrote: it is the leftmost, spoofable rule. No
  deployment gets that from a range it meant.
- **A range with address bits set past its prefix is refused** (`10.0.0.1/8`). It
  could mean the one proxy or the whole `/8`, and the two differ by sixteen million
  believed addresses. Python's `ipaddress` refuses it by default; nginx warns and
  masks. Writing the first address costs a keystroke; guessing wrong opens the list.
- **A mapped address is its IPv4 address everywhere**: the entry, the peer, each hop
  and the answer (`ClientOrigin`, `ClientIp`), the comparison in one function
  (`lists`), so no reader can forget it. The throttler's bucket read it so already.
  Two chains spelling one client two ways no longer read as a disagreement.
- **A range of one family never answers for the other.** `::/1` holds `::ffff:…` by
  its bits; matching by family keeps it from trusting every IPv4 peer.

**Refused:**

- **The `ipnet` crate.** The compare is one shift per family, and a public field holding
  its type would make it a public dependency (`public-dependencies.md`), for the one
  thing here it would do.
- **A `TrustedProxy::range(ip, prefix)` constructor beside the parse.** One way to
  write a range, the parse: `?` in a function returning `Result`, and in a `#[module]`'s
  imports, where no `?` reaches one, `.expect("a CIDR range")`, a typo failing that boot.
  `From<IpAddr>` stays, since a suite holds an address already.
- **`/0` as an explicit opening with a `warn`.** It opens what the list exists to
  close, and a deployment that forwards every address has no proxy to name.
- **Canonicalizing only for the comparison, leaving `ClientIp` mapped.** A developer
  comparing the answer with `203.0.113.7` would meet a spelling only the socket uses.

`Request::peer_addr` stays the socket's own answer, mapped or not.

## 2026-11-05: the chain is every field line

Review of the change above, from runs over a real socket and through the throttler: a
chain over two field lines read the first alone, so a caller rotating it behind a proxy
that adds its own line (RFC 7239 §4) answered `200` five times where the same hops on one
line answered `200, 200, 429, 429, 429`. Security decided:

- **`Forwarded` and `X-Forwarded-For` fold every field line, in order** (RFC 9110 §5.3);
  `X-Real-IP`, one hop, reads its last. The fold allocates only for a second line and only
  for a listed peer, so a caller off the list cannot make it allocate.
- **A quote that closes no value opens none.** Folding alone would have handed a caller's
  `x="` the proxy's line, and the hop appended to that same line it had already: run, it
  answered `Forwarded(198.51.100.99)`. A `quoted-string` starts a value and ends at a
  separator or the value's end, so no scan passes a later value's opening quote.
- **The browser-origin check's first value stays.** It says what the client asked for, and
  a value a caller forged can only claim an origin.

Refused: **parsing the lines apart behind a new `resolve` signature.** It would close the
two-line form and leave the one-line form open, for an API break.

Left, and named: a proxy appending to the caller's own line a quoted value of its own that
starts with a separator (`host=",a"`) still closes the caller's quote there. The bytes hold
the ambiguity; a proxy that strips the caller's line has none.
