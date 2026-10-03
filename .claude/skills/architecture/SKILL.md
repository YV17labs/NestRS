---
name: architecture
description: Architect-grade review of a named scope (a directory, a crate, a subsystem) — responsibility placement, layering, naming and namespace hierarchy, conformance to the repo's written rules and to public standards (W3C, RFCs, OWASP, OTel, …). Run when a territory opens or a scope has drifted, never on a diff. Agents argue and never fix; the fixes land afterwards, in the main thread, one at a time.
---

# `/architecture` — does this code live where it belongs?

`/audit` proves wrong answers with probes; `/simplify` cleans the current diff.
This skill asks the third question: **is each thing in the crate, module and
file that owns its concern, under the name and shape the rules mandate?** Its
findings are argued — from a written rule, a public standard, or an ownership
fact — rather than probed.

**When** (`CLAUDE.md`, *Reviews*): a territory opens — a new crate, decorator
family, vocabulary or `for_root` seam — or a scope has drifted over several
sessions. It takes a **scope**, never a diff, and runs before `/audit`, because
a move invalidates every probe taken before it.

**Agents argue; they never fix.** A placement finding moves a type across a
crate boundary — manifest, re-exports, every import — and three lanes editing in
parallel land three halves of three moves. Fixes are applied afterwards, in the
main thread, one at a time.

## The referential

A finding is strong exactly when it cites one of these, and noise when it cites
none:

- `CLAUDE.md`, `.claude/rules/architecture.md` (naming levels, role tables,
  folder law), and the zone rules whose `paths:` match the scope;
- `.claude/decisions/` — open the entry a rule cites before re-proposing what it
  records;
- **the public standards covering the concern** — W3C, IETF RFCs, OAuth/OIDC,
  OWASP, OpenTelemetry semantic conventions, JSON Schema, semver, the Rust API
  Guidelines. Prefer the standard; a deviation is legitimate when its argument
  is written where it lives, and a silent one is a finding.

## How to run it

1. **Scope it.** The argument is the scope. With none, use the crates the
   working tree touches. Refuse "the whole repo": propose successive scoped runs.
2. **Assemble the referential per lane.** Subagents do not auto-load zone rules:
   attach the rule files whose `paths:` match the lane's files, plus `CLAUDE.md`
   and `architecture.md`.
3. **Two to five lanes, by responsibility question** — not by file count. "Who
   owns each constant and target in this crate", "does the kernel know about any
   optional edge", "does every file match the role tables", "does this scope
   spell its subject with one word everywhere", "is every `pub` earned". One
   question, one scope.
4. **Give every lane the mandate below**, verbatim in substance.
5. **Triage into four buckets** — the bucket decides what happens:

   | Bucket | What it is | What happens |
   |---|---|---|
   | **Breach of a written rule** | the rule sentence quoted | **applied** — the rule already decided |
   | **Responsibility misplacement, argued** | a type, constant or check in a crate that does not own its concern, or a name off the tables | **applied** when one placement is argued and the other is not; when both hold an argument, decided by `CLAUDE.md`'s four criteria and reported with the evidence |
   | **Rule/code drift** | the prose says one thing, the code another | **applied the `CLAUDE.md` way**: the code wins and the prose is corrected in the same commit — unless the prose states a security invariant or a hard "no", in which case the code is fixed |
   | **Best practice, no local rule** | a practice no rule and no standard here mandates | **not applied.** State the practice and its cost, as an issue. A rule is added only on contradiction or recurrence (`CLAUDE.md`, *Reviews*) |

   A fix that needs something on a hard "no" list, or reopens a locked decision
   (test layout, workspace split, crate naming), stops and is asked, whatever
   its bucket.
6. **Report before applying.** Findings ranked by blast radius — a wrong-crate
   type outranks a wrong-module file outranks a naming nit — then, separately,
   what was examined and found conforming versus what was never examined.
7. **Apply sequentially, in the main thread**, largest blast radius first,
   because a moved type invalidates the findings that merely followed it. After
   each, the *Definition of done*'s local loop for what it touched; a batch compiled
   together hides which move broke. A fix that turns out to need a decision the
   review did not argue stops and is argued first — the argument is the licence.
   Then report what was applied, what was left as an issue, and what was never
   examined.

## The mandate each lane gets

> Review `<scope>` as a senior architect, against the attached rules **and the
> public standards covering the concern** — name the standard with the section
> a reader can check. **Argue, do not fix.** For every item, give exactly one
> of: the rule sentence it breaches, quoted; the standard it silently deviates
> from, named; or the fact that decides ownership — *who owns this answer?* —
> with both placements argued in two sentences each. A finding with none of the
> three is noise; drop it.
>
> Judge every public name as the path a caller types, and judge a scope's
> namespace against **everything it names at once**: directory, feature,
> re-export, span target, config namespace, README heading, files, and every
> exported type, trait, function, constant and error. One scope, one word — or
> the disagreement is the finding, reported once over the whole set with the
> count of sites on each side.
>
> For each finding: `file:line`, what it is, where it belongs and why, the
> blast radius if it stays, and every other file the fix touches (manifest,
> re-export, call sites). Distinguish "the constant is misplaced" from "the code
> it names is misplaced and the constant follows it" — the second is the
> finding. Where prose and code disagree, quote both sides.
>
> Do not fix, move or rename anything. Finish by naming what you examined and
> found conforming, and separately what you did not get to.

## What to hunt

- **A name that does not match its path.** First, on every scope: a plausible
  name reads fine alone and is only wrong against its location. Test both
  directions — from the path the type, from the type the path — over the whole
  set of siblings (`CLAUDE.md`, *Naming is the pillar*). A file whose path never
  says what it is gets the folder it should have been in, not a better name. A
  driver's stutter (`nest_rs::redis::RedisThrottlerModule`) is **correct**;
  flagging it costs the review its credibility.
- **A file that serves more than its edge folder.** An `http/`, `ws/` or `mcp/`
  folder states its file serves that edge, so a type the framework dispatches
  to at several edges makes the path false. **Open the file** — the name and
  `mod.rs` read correctly; the tell is what dispatches to the type. Also check a
  trait that is edge-bound without naming its edge, and an alias whose aliased
  type answers several edges. *Can every caller reach it without importing an
  edge it does not use?* The fix is the move, never a rename.
- **A concern in a crate that does not own it** — the kernel holding a
  descriptor for an optional edge, a transport holding a rule the kernel
  enforces. *Does the layering force the placement or merely excuse it?*
- **Knowledge leaking down.** A lower layer naming, matching on or
  special-casing something only an upper layer should know. The manifest's
  dependency direction is the easy half; vocabulary in the source is where it
  leaks.
- **A homemade answer where a standard exists** — an identifier, header,
  envelope, error shape or grammar a public standard already covers
  (`.claude/decisions/trace-context-over-request-id.md` is the precedent).
- **An interpreted string spelled as a literal** — a span target, env name,
  unit, queue name or error sentence the rules make a constant owned by its
  crate.
- **A name off the tables.** `*_module.rs`, an invented folder (`core/`,
  `shared/`, `types/`), a role suffix on vocabulary, a project or app name
  below its level, a `Service` with no domain logic, a vocabulary file whose
  stem reaches nothing it declares. Apply the tolerances `architecture.md`
  lists; a finding against them argues why they should not hold.
- **A synonym split.** Each path segment either narrows the one before
  (`http::HttpModule`) or names a second axis of it (`redis::RedisQueueModule`:
  *how*, then *what it binds*). A segment that is a second word for the subject
  above it makes neither half greppable from the other. Count the sites on each
  side before writing the finding — the count ranks the sides; where a public
  standard names the subject, the standard's word wins whatever the count; and a
  namespace also spells an env prefix a deployment reads, which weighs on the
  cost of each direction. One finding over the whole set, never one per name:
  applied piecemeal, it leaves both vocabularies present.
- **A second way to do one thing** — a seam, constructor, helper or spelling
  beside the sanctioned one, including a convenience wrapper that becomes the
  real API.
- **Speculative surface** — a `for_root` nobody calls, a `pub` nothing outside
  uses, a generic with one instantiation, an abstraction with one user.
- **Posture and security placement** — an authn/authz decision outside a guard,
  a denial below `warn`. Here the question is *where the decision lives*; whether
  it answers wrongly is `/audit`'s.
- **Prose the code contradicts** — a doc comment, rule or docs page stating
  behaviour that was true once. The drift bucket.

A finding provable by a probe — a wrong answer you can reproduce — belongs to
`/audit`; hand it there.

## Safety

- **Never `git checkout`, `git restore`, `git stash` or `git clean`.** The
  review may run against uncommitted work.
- **Never fix, move or rename during the review** — that is step 7's job, after
  the triage decides whether the finding applies.
- Scope every cargo invocation with `-p`; another process may share the target
  directory.

## When to stop

A scope whose findings keep coming back as *misplacements* has a boundary
problem, not a hygiene problem: the crate's mandate is unclear and the tenth
finding restates the first. Stop applying, and bring the boundary decision —
the mandate you propose, with its evidence — instead of the list. Fixing ten
symptoms of one unclear boundary spreads it. If the decision renames or merges
crates, it is the owner's: crate naming is locked.
