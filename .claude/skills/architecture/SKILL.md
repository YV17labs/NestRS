---
name: architecture
description: Architect-grade review of a named scope (a directory, a crate, a subsystem) — responsibility placement, layering, naming and namespace hierarchy, conformance to the repo's written rules and to public standards (W3C, RFCs, OWASP, OTel, …). Agents argue and never fix; the fixes land afterwards, in the main thread, one at a time. For existing code, not the current diff.
---

# `/architecture` — does this code live where it belongs?

`/audit` proves behavioral defects with probes; `/simplify` cleans the current
diff. This skill answers a third question neither asks: **is each thing in the
crate, module and file that owns its concern, under the name and shape the
rules mandate?** Its findings are rarely provable by a probe — they are argued
from a written rule or from an ownership fact — and the ones that are decided
by a written rule get **applied**, in the review's last phase.

**Agents argue; they never fix.** That split is mechanical, not ceremonial: a
placement finding moves a type across a crate boundary, which touches the
manifest, the re-exports and every import — three lanes editing in parallel
would land three halves of three moves. The fixes are applied afterwards, in
the main thread, sequentially, *Definition of done* between each.

## What makes it work here

This repo has an unusually complete written referential: `CLAUDE.md`,
`.claude/rules/architecture.md` (naming levels, role tables, folder law), and
the zone rules in `.claude/rules/`. **A finding is strong exactly when it
cites one of those sentences or names the fact that decides ownership.** A
finding with neither is taste, and taste is noise.

**The referential also includes the public standards that cover each lane's
concern** — W3C, IETF RFCs, OAuth/OIDC, OWASP, OpenTelemetry semantic
conventions, JSON Schema, semver, the Rust API Guidelines. The reviewer often
knows a standard the author did not; naming it is part of the job. The repo's
own posture (recorded in `CLAUDE.md`) is the bar: prefer the standard, and
when deviating, state the deviation and its argument in writing — a homemade
`request_id` was retired for W3C Trace Context on exactly this reasoning, and
the three deliberate deviations from OTel's log model are each argued where
they live. A deficient standard may be declined and a better local design
adopted — but only with the deficiency named; a silent deviation is a finding.

## How to run it

1. **Scope it.** The argument is the scope — a directory, a crate, a
   subsystem. With no argument, use the crates the working tree touches.
   Refuse "the whole repo" politely: propose a split into successive scoped
   runs instead, because a lane given everything checks everything shallowly.

2. **Assemble the referential per lane.** Subagents do not auto-load zone
   rules. For each lane, attach (or name for reading) the rule files whose
   `paths:` match its files — `framework.md` for `crates/nest-rs-*`,
   `features.md` for `demo/crates/features`, and so on — plus `CLAUDE.md`'s
   hard-no list and observability section, and `architecture.md` always.

3. **Split into two to five lanes, by responsibility question — not by file
   count.** Good lanes: "who owns each constant and target in this crate",
   "does the kernel know about any optional edge", "does every file match the
   role tables", "does this scope spell its subject with one word everywhere",
   "is every `pub` earned". A lane is one question asked of one scope.

4. **Give every lane the mandate below, verbatim in substance.**

5. **Triage yourself, into four buckets** — the bucket decides whether the
   finding is applied or asked, so never blur them:

   | Bucket | What it is | What happens |
   |---|---|---|
   | **Breach of a written rule** | the rule sentence quoted | **applied** — the rule already decided |
   | **Responsibility misplacement, argued** | a type, constant or check in a crate that does not own its concern, or a name off the tables | **applied** when one placement is argued and the other is not; **asked** when both placements hold an argument |
   | **Rule/code drift** | the prose says one thing, the code does another | **never applied.** Report both sides, edit **neither** (`CLAUDE.md`, *Autonomous work*) — applying one side chooses in silence which of the two was right |
   | **Best practice, no local rule** | a practice no rule and no standard here mandates | **never applied.** State the practice and its cost. If the owner adopts it, it becomes a sentence in `.claude/rules/` — and the *next* run applies it as bucket one |

   A finding whose fix would need something on the *Hard "no" list*, or would
   reopen a locked decision, halts and goes to the owner whatever its bucket —
   that is `CLAUDE.md` law, not an exception this skill grants.

6. **Report before applying.** Findings ranked by blast radius (a wrong-crate
   type outranks a wrong-module file outranks a naming nit), then — separately
   — what was examined and found conforme versus what was never examined.
   Those are different facts and the owner needs both.

7. **Apply, sequentially, in the main thread.** One finding at a time, largest
   blast radius first, because a wrong-crate type moving invalidates the
   findings that merely followed it. After each fix, the *Definition of done*
   for every workspace it touched — a move that does not compile is not a fix,
   and a batch of six moves compiled together hides which one broke. A fix that
   turns out to need a decision the review did not argue **stops and asks**
   rather than improvising: the argument is the licence, and there is no fix
   without one.

   Then say, separately: what was applied, what was reported and left, and what
   was never examined.

## The mandate each lane gets

> Review `<scope>` as a senior architect, against the attached rules **and the
> public standards covering the concern** (W3C, IETF RFCs, OWASP, OTel
> semantic conventions, Rust API Guidelines, …) — name the standard when you
> invoke it, with the section a reader can check. **Argue, do not fix.** For
> every item you flag, give exactly one of: the rule sentence it breaches,
> quoted; the standard it silently deviates from, named; or, where neither
> decides it, the fact that decides ownership — *who owns this answer?* — with
> both placements argued in two sentences each. A finding with none of the
> three is noise; drop it.
>
> Judge every public name in the form a caller types it — the fully-qualified
> path, not the bare item — and judge a scope's namespace against **everything
> it names at once**: its directory, its feature, its re-export, its span
> target, its config namespace, its README heading, its files, and every type,
> trait, function and constant it exports. One scope, one word, or the
> disagreement is itself the finding — reported once over the whole set, with
> the count of sites on each side, never one finding per name.
>
> For each finding: `file:line`, what it is, where it belongs and why, and the
> blast radius — what drifts, breaks or misleads if it stays. Distinguish "the
> constant is misplaced" from "the code it names is misplaced and the constant
> merely follows it"; the second is the real finding and the first is its
> shadow. Name the other files the fix would have to touch — the manifest, the
> re-export, the call sites — because the one applying it works from your list.
>
> Do not fix, move or rename anything. Do not edit prose to match code or code
> to match prose — report the drift with both sides quoted.
>
> Finish by naming what you examined and found conforme, and separately what
> you did not get to. The owner needs to tell "clean" from "not looked at".

## What to hunt — the architect's classes

Each class below has already produced a real question or defect in this repo.
Hunt these before anything generic.

- **A name that does not match its path.** Check this *first*, on every scope,
  because it is the class an LLM most reliably under-weights: a plausible name
  reads fine in isolation and is only wrong against its location. The property
  to test is bidirectional — from the path you must know the type, from the type
  you must know the path — and the unit is the whole set of siblings, never one
  name. `redis/queue/module.rs` ⇒ `RedisQueueModule`, `audio/http/module.rs` ⇒
  `AudioHttpModule`, `posts/http/controller.rs` ⇒ `PostsController`. A mismatch
  is not a nit: it is the one defect that makes every *other* review harder,
  because a reader who cannot navigate cannot check anything else. The finding
  that follows is **a file whose path never says what it is** — a
  `nest-rs-redis/src/module.rs` holding a bare `QueueModule`, where the fix is
  the folder the file should have been in, not a better name.

  **A driver's stutter is not that finding, and this paragraph used to say it
  was.** `nest_rs::redis::RedisThrottlerModule` repeats `redis` at the path and
  is **correct** — `CLAUDE.md` settles it: "the stutter at the path is an
  accepted cost: a name that is unambiguous in a log outranks a name that is
  short in an import." Flagging it costs the review its credibility on the one
  law it exists to enforce. `nest-rs-conformance`'s `naming.rs` mechanises the
  module, the adapter, the edge-folder and the binding-folder cases; everything
  else here is yours.

- **A file that serves more than the folder it sits in.** The sibling of the
  class above, one level down, and the one that hides best: an edge folder
  (`http/`, `graphql/`, `ws/`, `mcp/`, `queue/`, `schedule/`, `events/`) states
  that its file serves that edge, so a file answering two from inside one makes
  its own path a false statement. **Open the file — this class is invisible from
  the outside.** The module list, the `mod.rs` and the type name all read
  correctly; the tell is inside, and it is that the framework dispatches to the
  type at edges the folder does not name. `nest-rs-authz/src/http/guard.rs` held
  a guard implementing `check_http`, `check_graphql`, `check_ws_message` **and**
  `check_mcp` for a full release. `AbilityGuard` was a good name the whole time.
  The blast radius is never just the path: three transports had to enable the
  `http` feature to reach their own guard, the WS entry compiled under `http`,
  and three of the demo's four `Authz<Edge>Module`s imported an HTTP adapter they
  never served. *Ask: what dispatches to this type, and can every one of those
  callers reach it without importing an edge it does not use?* The fix is the
  move — up to the level every answering edge reaches — never a better name.
  `naming.rs`'s `no_file_under_an_edge_folder_answers_another_edge` mechanises
  the part that is a symbol; **two parts are yours**, because a scan on
  identifiers cannot see them: a trait that is edge-bound without naming its edge
  (`SocketContext`, `RouteResponseShaper`), and an alias whose *aliased* type is
  what answers several edges.
- **A concern in a crate that does not own it.** A type, constant, check or
  descriptor whose subject belongs to another crate — the kernel holding a
  descriptor for an optional edge, a transport holding a rule the kernel
  enforces. *Ask: who owns this answer, and does the layering force the
  placement or merely excuse it?* The layering reason, when real, is the
  finding's answer — name it.
- **Knowledge leaking down.** A lower layer naming, matching on, or special-
  casing something only an upper layer should know exists. Dependency
  direction on the manifest is the easy half; vocabulary direction in the
  source is where it actually leaks.
- **A homemade answer where a standard exists.** An invented identifier,
  header, envelope, id format, error shape or grammar covering ground a public
  standard already covers — trace context, problem details, OAuth flows,
  OTel attribute names, semver ranges. The precedent is the repo's own:
  `request_id` retired for W3C Trace Context, argument recorded. *Ask: does a
  standard answer this — and if we deviate, where is the deviation argued?*
  Declining a deficient standard is legitimate; doing it silently is the
  finding.
- **An interpreted string spelled as a literal.** Span targets, env names,
  unit names, queue names, error sentences — the rules make each a constant
  declared by its owner. A literal is both a typo surface and a second
  authority.
- **A name off the tables.** `*_module.rs`, an invented folder (`core/`,
  `shared/`, `types/`), a role suffix on vocabulary, a project or app name
  below its level, a `Service` that owns no domain logic.
- **A name outside its namespace or hierarchy.** A name is read *with* its
  path, and the two spell the meaning exactly once: an item restating its
  module (`target::TARGET_ACCESS_GRAPH`) is redundancy, an item ignoring it is
  an orphan the grep never finds. Siblings at one level follow one scheme —
  one odd member means either it or the scheme is wrong, and which one is the
  finding. And every level of the hierarchy (project → crate → module → file →
  item) names exactly its own level: a level skipped, restated below its
  floor, or overloaded with two meanings breaks the property the naming law
  buys — that "where is this?" and "what is this?" have the same answer.
  Where the tables are silent, this principle still binds.

  **Read the qualified path, not the item.** Every public name is judged as the
  string a caller types — `nest_rs::oauth_discovery::OAuthResourceModule`,
  never the bare `OAuthResourceModule` — because that string is the only
  form anyone ever reads, and a name that is fine alone can still be wrong at
  the end of its path. Each segment does exactly one of two things to the one
  before it: it **narrows** it (`http::HttpModule`,
  `seaorm::health::DatabaseHealthModule`), or it names a **second axis** of it —
  the one sanctioned pair being an implementation namespace over a port type
  (`redis::RedisQueueModule`, `seaorm::SeaOrmDatabaseModule`), where `redis`
  says *how* and `Queue` says *what it binds*. A segment that does
  neither — a second word for the subject the segment above already named — is a
  **synonym split**, and it is the costliest naming defect there is: neither
  half is greppable from the other, so a reader has to know both words to find
  either, and every item the crate adds inherits the split.
  `nest_rs::oauth_discovery::OAuthResourceModule` **was** that defect in the
  framework's own front door — the namespace called the concern *discovery*, the
  type called it *protected resource*, and the crate's `//!` argued at length for
  the first word while every single item it exported was spelled in the second.
  It has since been settled the other way: the crate is `nest-rs-oauth-resource`,
  the target `nest_rs::oauth::resource`, the namespace `oauth_resource`, and the
  exports `OAuthResource*` — one word at every site. The worked count below is
  kept because the *method* is what transfers, not because the defect is live.
  The finding is never "the type is badly named": it is **the two levels
  disagree**, one of them has to move, and arguing *which* — with the cost of
  each direction — is what you owe.

  **And it is every name, not the module's.** `*Module` is merely the one a
  reader types first; the split is in the whole vocabulary or it is nowhere.
  So the unit checked is **one scope, one word**, and the sites are all of
  these at once — the directory, the umbrella feature, the umbrella re-export,
  the `TARGET` span target, the `#[config(namespace = ..)]` that becomes the env
  prefix, the README's `# ` line, the file names, and **every exported type,
  trait, function, constant and error**. Count them before you write the
  finding, because the count *is* the argument for which side moves:
  `nest-rs-oauth-discovery` spelled *discovery* at five of those sites (directory,
  feature, re-export, `TARGET`, config namespace — hence `NESTRS_OAUTH_DISCOVERY__*`
  on the deployment) and *protected resource* at all four it exported
  (`OAuthResourceConfig`, `ProtectedResourceMetadata`, `OAuthResourceModule`,
  `OAuthResourceSetup`). Five against four is not a tie, and neither number
  is a `*Module` count.

  **The count ranks the two sides; it does not decide them.** Where a public
  standard names the subject, the standard's word wins whatever the count says
  — RFC 9728 is titled *OAuth 2.0 Protected Resource Metadata*, so renaming
  `ProtectedResourceMetadata` to agree with its namespace would trade this
  finding for the next class down this list, *a homemade answer where a
  standard exists*. The count decides only where no standard has an opinion,
  and the cost of each direction is the tiebreak below that: a namespace also
  spells an env prefix, and a deployment reads that one.

  Report it as **one finding over the whole set**, never one per name: a
  namespace and its items are a single decision, so nine findings are nine
  copies of one, and applying them one at a time renames the crate into an
  intermediate state where the two vocabularies are *both* present. The same
  test runs at every level, not just the crate — a feature folder against the
  types inside it, a module folder against its items, a `dtos/` against the
  boundary it is named for.

- **A second way to do one thing.** A second seam, constructor, helper or
  spelling beside the sanctioned one — including a convenience wrapper that
  quietly becomes the real API.
- **Speculative surface.** A `for_root` nobody calls, a `pub` nothing outside
  uses, a generic parameter with one instantiation, an abstraction with one
  user. Visibility wider than its use is a promise nobody priced.
- **Posture and security placement.** An authn/authz decision outside a guard,
  a denial below `warn`, a path that returns a value where an error is honest.
  These overlap `/audit`'s classes deliberately: here the question is *where
  the decision lives*, there it is *whether it answers wrongly*.
- **Prose the code contradicts** — a doc comment, rules file or docs page
  stating behaviour that was true once. Report the drift; never resolve it.

A finding that turns out to be provable by a probe — a wrong answer you can
reproduce — belongs to `/audit`; hand it there rather than arguing it here.

## Safety

- **Never `git checkout`, `git restore`, `git stash` or `git clean`.** The
  review may run against uncommitted work.
- **Never fix, move or rename during the review** — that is phase 7's job, and
  an agent doing it early pre-empts the triage that decides whether the finding
  was ever applicable.
- **Never edit prose to match code, or code to match prose**, in any phase.
  Drift is reported with both sides quoted and resolved by the owner.
- Scope every cargo invocation with `-p`; another process may be compiling in
  the same target directory.

## When to stop

A scope whose rounds keep surfacing *misplacement* findings has a boundary
problem, not a hygiene problem — the crate's mandate is unclear, and the tenth
finding restates the first. Say so and put the boundary question to the owner
instead of finishing the list, and apply nothing: fixing ten symptoms of one
unclear boundary spreads the boundary, it does not settle it.
