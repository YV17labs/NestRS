# A modular monolith, deployed per workload, and the objections already answered (7.0)

The owner asked for the architecture to carry a name (2026-10-03): the domain
written once as modules, deployed as one binary per workload by default, or as
one binary from the same modules. The canonical text is the *A modular monolith,
deployed per workload* section of `docs/src/content/docs/why.mdx`, drawn by the
`Topology` component. Open this entry before re-raising any objection below.

**Service Weaver is the goal, not the mechanism.** Google's Service Weaver
(announcement 2023-03-01; *Towards Modern Development of Cloud Applications*,
HotOS '23) states the goal, the development velocity of a monolith with the
scalability, security and fault tolerance of microservices, and the diagnosis:
microservices conflate how code is written with how it is deployed. NestRS
shares both. It takes neither the runtime that places components on processes
nor the RPC generated between them: a module is linked into every binary that
uses it, and binaries coordinate through the queue and the database. "100%
Service Weaver" was refused as false, and a difference is written as the choice
it is, never as a gap. Service Weaver stopped development on 2024-12-05; the
docs keep the date and drop the stated reason (adoption meant rewriting), which
a reader turns against a framework that is a rewrite too.

The objections, and the answers settled:

- **A database shared between services is an anti-pattern.** It targets
  services that separate teams release separately. Inside one workspace the
  binaries are one application, one schema, one migrations crate: a monolith
  run as several process types (Sam Newman: a monolith is a unit of deployment),
  not a distributed monolith, since no binary calls another.
- **The binaries cannot release independently.** They can. The line is the
  database, the owner's rule: binaries sharing a database live in one workspace
  and release from one commit; an app needing a database of its own becomes a
  project of its own, with its own workspace, repository and release.
- **Separate repositories lose compiler-checked contracts.** False. What two
  projects share goes through a library crate both depend on, so each compiles
  against the contract; separate releases only mean each pins its own version,
  and the library's semver says when they drift. Any crate shares this way
  (path, git, registry) while both sides are on the same NestRS major.
- **Two releases coexist during a rolling deploy.** The framework versions its
  queue envelope: a job a newer release sealed is handed back unread until a
  consumer of that release runs it. A payload field or a column the app changes
  follows expand-then-contract.
- **Every binary holds the same database credentials.** Each binary reads its
  own `<PREFIX>_SEAORM__URL`, so each app connects as its own database user with
  only the rights it needs. No framework module changes the schema at boot (the
  only `create_table` outside tests is the CLI's migration template, checked
  2026-10-03), so a restricted user boots; only the migration job needs the
  owner's rights.
- **It cannot run as a plain monolith.** It can. A throwaway binary importing
  every edge of every Publish app (2026-10-03) booted and served HTTP, GraphQL,
  OpenAPI, MCP, the token issuer, two WebSocket gateways, the queue workers and
  the schedules. One collision: Publish's `users` controller and gateway both
  mount `/users`, and in one binary the boot refuses a duplicate path and names
  both, so merging Publish takes one rename; the figure says so.
- **Calls between projects are hand-managed, against the thesis.** By design:
  the framework never makes a network call look like a method call, and ships
  no outbound HTTP module (a hard "no" in `CLAUDE.md`). An app calling another
  carries that call itself, from authentication to retries; the W3C propagator
  is installed, and injecting it into the app's own client is the app's. The
  channel between projects the framework carries is the queue.
- **Every binary links all the features code.** It does, and what a binary does
  not import is never mounted, so it is unreachable. Splitting the features
  crate by Cargo feature would shrink the binaries: an issue, not a defect.
- **Several databases in one binary.** Not offered: `Repo` holds one connection
  per binary. A second database is a second project, by the line above.

Open (2026-10-03): the `Topology` figure's data is kept by hand, so a demo change
can make it false silently. A docs lint rule checking it against
`demo/apps/*/src/module.rs` is the planned fix.
