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

## 2026-11-07: a header is read whole or not at all

The residual named above, run: `Forwarded: for=198.51.100.99;x=", for=203.0.113.7;host=",a"` from
a listed peer answered `198.51.100.99`, the caller's hop. Two more members answered it too: a hop
the parser skipped handed the client's position to the hop on its left, the caller's
(`X-Forwarded-For: 198.51.100.99, garbage`; `Forwarded: for=198.51.100.99, for=unknown`, the proxy
withholding the client), and an unreadable `Forwarded` beside a caller's `X-Forwarded-For` left
the answer to the latter. Security decided:

- **A chain the parser cannot read whole is no chain, and the proxy stands** (`TrustedProxy`),
  whatever the other headers say, with one `warn` naming the header and the peer, never a hop,
  filed inside the request once its trace exists. `X-Real-IP`, one hop in no order, is read only
  where neither chain names a client, and refused the same way there: beside a chain the proxy
  appended to, an unreadable one can only be a caller's.
  The grammar refuses what reads two ways: a quote no quote closes, a quote inside a value, a
  closing quote followed by anything but a separator, a pair with no `=` or no name, an empty
  value, a hop that is no node (RFC 7239 §6), bytes no text holds. A proxy's appended part is
  well-formed, so with the caller's quote left open its quotes shift by one and the value ends
  inside a string: the refusal is certain, not heuristic.
- **A client position holding `unknown` or an obfuscated identifier names no client**: what sits
  left of it, or in another header, is the caller's own to write.
- **Whitespace and characters outside `tchar` in an unquoted value pass**: they read one way, and a
  hand-written proxy rule emits `for=[2001:db8::1]:8080` unquoted.
- **`ClientOrigin::of` returns the edge's resolution**, so a request files one line, not one per
  reader (`ClientIp`, the throttler). The edge resolves before the request's trace exists, so the
  resolution hands back what it refused, and the edge files it inside the request's span and
  correlation.

Refused:

- **Reading only the unreadable header as absent.** Behind a proxy appending to `Forwarded` and
  passing `X-Forwarded-For` through, the caller's own `X-Forwarded-For` would decide.
- **Reading a chain from the right and stopping at the client**, which would read the residual
  right and never look at a caller's bytes. It is safe for `X-Forwarded-For`, and holds for
  `Forwarded` only on a backslash-parity scan from the right; one rule for the three headers, read
  whole, costs a caller sending a malformed header of its own the proxy's bucket and nothing else.
- **Refusing the request.** The client's address is observational (`ClientIp`'s own caveat); the
  request is served as the proxy's.

## 2026-11-07: a list holding a whole family is a `/0`, and a literal is checked as it compiles

Run: `0.0.0.0/1,128.0.0.0/1` booted, believing every IPv4 caller as `/0` does, and the spelling
this entry gave for a `#[module]`'s imports, `"10.42.0.0/61".parse().expect("a CIDR range")`,
panicked at the boot. Security and the developer decided:

- **Ranges holding every address of a family between them are refused at boot**, from the
  environment and from code alike (`HttpConfig`'s `trusted_proxies` reader), naming the variable
  or the field and never an entry. The check is a sort and a sweep over each family's spans, once.
  A list missing only the mapped `::ffff:0:0/96` still believes every IPv6 caller and boots: no
  one writes it by accident, and the refusal states a fact rather than a threshold.
- **A literal is `trusted_proxy!("10.42.0.0/16")`**, which reads the text as the macro expands
  and builds the entry in a constant item, through the same `const fn` checks `FromStr` runs: a
  refusal is a compile error in the boot's words, which `cargo check` and an editor show. An
  inline `const { }` would wait for a build. This revises the refused "range constructor beside
  the parse": the text is still the one way to write a range, and the parse still the one
  checker; the macro only moves a literal's check to compile time. `ip.into()` stays for an
  address a value holds, and `.parse()?` for text read at run time.

Refused: **a `const fn` parsing the text itself**, which would re-implement the standard library's
address grammar for one literal; **a range's breadth past the whole family** (a `/1` alone): where
to draw a line short of every address is the deployment's statement, not a fact.

## 2026-11-07: what no caller connects from counts as held

Run: `0.0.0.0/1,128.0.0.0/2,192.0.0.0/3` booted, believing every unicast IPv4 caller; what it left
out, `224.0.0.0/4` multicast and `240.0.0.0/4` reserved (RFC 1112 §4), no caller reaching a server
holds. In IPv6, eight entries short of `ff00::/8` did the same. Security decided: the sweep counts
as held, beside the list's own spans, what no caller connects from — those two IPv4 ranges, IPv6's
multicast `ff00::/8` (RFC 4291 §2.7), and the mapped `::ffff:0:0/96`, which a peer reads as the
IPv4 address it carries. This revises the entry above: a list missing only the mapped range is a
whole family too, and the refusal still states a fact, now of what a caller can be.

Refused: **a list holding every globally routable address** (all of the public space, short of a
private range): it reads IANA's special-purpose registry, which moves, and such a list is written
range by range, never by accident; it stays the deployment's statement.

## 2026-11-08: `X-Real-IP` names the chain's client, or none is believed

Review of the 2026-11-07 header entry, run through the throttler: behind a listed proxy writing
`X-Real-IP` (nginx's `proxy_set_header X-Real-IP $remote_addr`, with no `X-Forwarded-For` line) and
passing the caller's own `X-Forwarded-For` through, five requests rotating that line answered `200`
five times where the route allows two. The chain named a client, so `X-Real-IP` was never read, and
the caller chose its bucket. It is the disagreement two chains are already refused on (RFC 7239
§8.1), for the same reason, with the third header left out. Security decided, revising that entry:

- **The headers a listed peer relays agree, or none is believed.** Beside a chain naming a client, a
  readable `X-Real-IP` naming another address off the list is a disagreement: the proxy stands, with
  the one `warn` two chains file. One naming the same client, in any spelling, or a hop of the list
  changes nothing.
- **`X-Real-IP` is read whole wherever it is sent**, as the chains are: an unreadable one beside a
  chain is refused with its `warn`, and `unknown` or an `_id` is a client the proxy withheld. The
  entry above read it only where no chain named a client, since an unreadable one there can only be a
  caller's; so can a readable one, and one rule for the three headers is the one a reader keeps.

The cost is the chains' own: a caller sending an `X-Real-IP` of its own through a proxy that passes it
on counts in that proxy's bucket, which a malformed `X-Forwarded-For` already earned it. A proxy
writing `X-Real-IP` beside an `X-Forwarded-For` a hop off the list wrote before it reads every caller
as the proxy, said by that `warn`: the deployment lists that hop, or has the proxy append.

Refused: **preferring `X-Real-IP` over a chain**, which would answer right behind the nginx above and
let the caller choose behind a proxy appending to `X-Forwarded-For` and passing `X-Real-IP` on: no
header says which one the proxy wrote.
