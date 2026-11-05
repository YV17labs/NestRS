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
