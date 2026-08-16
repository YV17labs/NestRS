# NestRS docs — style & structure norm

This file is the **single source of truth** for how docs pages are written. It exists because the
corpus was authored across many LLM/human sessions and drifted into dialects. The norm lives in
the repo — enforced by `docs/scripts/lint-docs.mjs` in CI — so a new session cannot ship a new
dialect unnoticed. When in doubt on any page, apply these rules.

On conflict about docs prose, this file wins; on conflict about code or naming, `CLAUDE.md` wins.
Where a rule is *derived* from the framework's own source (§F), the source wins over both — the
linter reads it rather than restating it.

## The goal

The docs must be the best on the market for **developers and software architects** evaluating or
implementing the framework. Four operating rules:

1. **Make them want it (SELL).** Reinforce the thesis — *you write business logic; the framework
   carries the rest* — with working code and verifiable evidence, not adjectives.
2. **Simple first (PATH).** The 80% case in the first screen of every page. Complexity is allowed,
   but always *behind* the simple case — progressive disclosure, advanced material marked as
   advanced.
3. **Never repeat — link (DRY).** Every concept has exactly ONE canonical page. Other pages get
   one sentence plus a link.
4. **Intuitive structure.** Categories and ordering follow the reader's journey, not the crate
   layout.

## A. Controlled H2 vocabulary

Structural section headings use **only** these names, in canonical order where present:

`Install` → `Run it` → `Wire it in` → *(page-specific content sections)* → `Configuration` →
`Limits` → `What fails if you get it wrong` → `Reference` → `Going further`

Page-specific *content* headings are free. Structural blocks use only the controlled names.

**Banned heading variants** (normalize on sight):

| Banned | Use instead |
|---|---|
| Wiring it up, Wire it into the app | Wire it in |
| Where to go next, Next steps, See also, Going deeper | Going further |

The normative closing block is **`## Going further`** (the majority convention). Utility/terminal
pages are exempt (see the linter's exempt list): `404`, `glossary`, `decorators`, env-var
reference.

## B. One template per page type

- **T-CONCEPT** (reference/concept page, the majority type): frontmatter (`title`, one-sentence
  `description` stating the single question the page answers) → opening paragraph (what you'll
  have at the end, ≤ 3 sentences) → first working snippet (≤ ~15 lines, **no Aside above it**) →
  `Install` + `Wire it in` (if applicable) → the 80% case → variations → `### Advanced`-gated
  material → `Limits` (one consolidated section) → `Going further` (2–4 links).
- **T-INDEX** (section landing): opening paragraph → minimal end-to-end example → "In this
  section" list (matching sidebar order) → `Going further`.
- **T-TUTORIAL** (tutorial step): goal sentence → numbered `<Steps>` each ending with expected
  output → one "what just happened" paragraph → link to the owning reference page → `Going
  further` pointing to the next step only.
- **T-RECIPE** (how-to, add-login shape): problem statement → prerequisites (one line) → numbered
  steps with checkpoints → `What fails if you get it wrong` → `Going further`.
- **T-SINGLE** (single-page section like server-timing): T-CONCEPT with `Install`/`Run it`
  mandatory in the first screen.

Skeletons live in `docs/templates/`.

## C. Component conventions

- `<Aside type="tip">` = optional shortcut; `note` = context the reader may skip; `caution` =
  footgun with consequences. **≤ 3 Asides total per page.**
- `<Steps>` for any numbered procedure.
- `<Tabs syncKey=…>` only for genuine alternatives (workspace/standalone).
- Code fence titles: a `title="…"` naming a real repo path must match that file byte-for-byte or
  say "(abridged)". Fictional examples get generic `src/…` titles, never a real-looking repo path.
  Fence titles cite the **user's** workspace shape (`crates/features/…`); GitHub URLs use the real
  repo paths (`demo/crates/features/…`).
- Terminal transcripts: `$`-prefixed input lines, trimmed output (≤ ~8 meaningful lines), no
  fabricated sequencing (a log line never appears before the command that causes it).
- One `Piped` destructuring style, one boot-log format across pages.

## D. The anti-drowning charter (simplicity is a budget)

1. **Page budgets.** A reference page: ≤ ~250–300 lines, answers **one question** (the one its
   frontmatter description states). A tutorial page: ≤ ~250 lines, ends on a runnable checkpoint.
   Per page: ≤ 3 Asides; scattered cautions consolidate into **one `Limits` section**; the first
   screen is one working snippet (≤ ~15 lines) with **no Aside above it**.
2. **Evidence placement.** Proof follows the promise it proves. Never a failure demo before the
   reader's first success. Boot/compile errors live under `What fails if you get it wrong` *after*
   the 80% case. Verbatim outputs are real (run it once, paste it), trimmed to ≤ ~8 lines. **Each
   evidence artifact appears once site-wide** — every other page links to it.
3. **Competitor mentions.** Named competitors (NestJS, BullMQ, Socket.IO, Sidekiq, Hasura…) appear
   **only** on the landing, `why.mdx`, and the comparison page. Reference pages sell by
   demonstration.
4. **Prose style charter.** Second person, present tense, active voice. Average sentence ≤ ~22
   words. **Banned words** (the linter greps): *blazing(ly), powerful, seamless(ly), simply,
   effortless(ly), easy, magic(al)*. **No exclamation marks in prose.** The voice is a calm senior
   engineer showing you something that works — never a brochure.
5. **Table-vs-prose.** Tables only for parallel lookup facts (≥ 3 rows, comparable columns).
   Decisions and narratives stay prose. No single-row tables.
6. **Link discipline.** Glossary link on first use per page only, never in headings or code
   captions; ≤ ~2 inline links per paragraph outside `Going further` blocks.

## E. The example canon — one universe

One product universe — **Publish** — with one canonical feature per concern. Never invent a
feature. A docs example is either (a) a quote/abridgement of a real demo file (fence title = real
path, "(abridged)" when trimmed), or (b) a minimal fictional snippet **inside the canon domain**
with a generic `src/…` title.

**The one escape — a concept with no canon home.** Some pages teach a shape the Publish universe
has no feature for: the app's own claims module, an external service you depend on. Those name a
**neutral placeholder** rather than a second product (`identity`, which is also what `nestrs g
auth` scaffolds; `upstream` for a third-party dependency). The test is whether the canon *could*
have carried it: a pure calculation, a CRUD slice or a migration walkthrough always can, so it
takes `posts` / `users` / `orgs` and inventing a name there is the violation this rule names. The
linter greps a ban list and cannot see this — it is a review call.

**Ban list** (the linter greps; must return zero): the identifiers `ItemsService`,
`ProductEntity`, `artworks`, `file_assets`, `Ledger` — plus the *shapes* an off-canon feature
leaks in as, whatever noun it picks: an `Item`/`Product`/`Order` role type
(`OrdersController`, `ProductService`), a `path =`/`title =` under `/items`, `/products`,
`/orders`, and a route attribute on one. Bare `items`/`products`/`points` as English are
deliberately not greped — too many false positives; they are a review call, like the escape above.

| Docs area | Canonical example |
|---|---|
| Landing, Getting started | `hello` (greeting) |
| Tutorial + Fundamentals | `blog` app, `posts` feature |
| HTTP, Validation, Database, Pagination | `posts` |
| Relations, row-level, masking, by-id | `users` + `orgs` |
| Security (authn/authz) | `users`/`orgs` + the `auth` app |
| GraphQL | `users` (+ `org` relation) |
| WebSockets | `chat` / `notify` (`demo/apps/live`) |
| Queue + Schedule | `audio` / `TranscodeCommand` (`demo/apps/worker`) |
| Events | `PostPublishedEvent` (notifications listener) |
| MCP | `weather` (+ `hello` tool) (`demo/apps/assistant`) |
| OpenAPI, Health, Rate limiting, OTel, Testing | the `api` app over `users`/`posts` |
| Storage | the `audio` slice's uploads (`demo/crates/features/src/audio`) |

## F. Code truth — the checks the prose rules can't see

Style is half the job; a page that reads well and does not run is still a defect. Each of these
was filed against a shipped release by a reader following a page verbatim, so the linter now
greps for them:

- **`version-pin`** — a literal `nest-rs* = "X.Y"` (either manifest form) must match
  `[workspace.package] version` in the repo root `Cargo.toml`, which is also what
  `nestrs g resource` writes. Bump the release, bump the pages — or use `workspace = true`,
  which carries no version at all.
- **`bind-order`** — the by-id binder takes its **action marker first**, entity second:
  `Bind<Read, PostEntity>`, and the proof it returns is `Authorized<Read, PostEntity>`. The
  reversed spelling reads plausibly and does not compile, so a page that repeats it teaches the
  wrong rule; ~10 pages shipped it reversed in 1.1.1. Gated rather than trusted.
- **`queue-name`** — a queue is named by its `QueueName` **type** on both sides. The consumer's
  `#[process(queue = "audio")]` is a compile error the macro raises by name, and the producer's
  string-taking `push(name, job)` is the runtime-name hatch, not the default — `push_to::<Q>` is.
  Both spellings shipped in 1.1.1 across ~10 places, on pages that predated `QueueName`.
- **`architecture-drift`** — `architecture.mdx` restates a file the CLI embeds
  (`nest-rs-cli/src/templates/architecture.md`, symlinked into `.claude/rules/`), so the page is
  diffed against it: the role/file table and the reserved-vocabulary list must name the same
  tokens. A rule the scaffolded project ships and the docs contradict is worse than an undocumented
  one. Add a page to `MIRRORED_PAGES` when it starts restating a shipped file.
- **`unauthed-curl`** — a `curl` naming a concrete host and a guarded REST root (`/posts`,
  `/users`, `/orgs`, …) carries an `Authorization` header. The guards run before the pipe and
  before the handler, so a token-free call documents a `401` the page never mentions. A block
  demonstrating the denial (`401`/`403` in its own output) is exempt — that is the point of it.
  `/graphql` is out of scope: one endpoint, per-operation posture.
- **`crud-error`** — a **handler** snippet must not `?` a `CrudService` read (`list()`, `page(`,
  `access(`). Those return `Result<_, DbErr>`, and `DbErr` is not a `ResponseError`: the line
  does not compile. The fix is a layering one, not a `map_err` at the route — the exemplar's
  services return the **wire type** (`demo/…/posts/service.rs`: `create_in_org` → `Post`), so a
  hand-written handler is a one-line delegation and the `Model` → wire conversion plus the
  `ServiceError` mapping live in the service. Only handler blocks are checked; a service body
  converting `DbErr` through `?` is the correct shape.
- **`install-stanza`** — a page that publishes its install list twice **under `## Install`** (a
  `cargo add` line in a `bash` block, a `[dependencies]` block in `toml`) must have the two say
  the same thing: same
  crates, same features, same `default-features`, and an explicit `@<req>` on the `cargo add`
  whenever the manifest constrains past the major. The reader runs the bash line first, so the
  half that drifts is the half that breaks: 1.3.0 shipped `cargo add validator` (resolving 0.21)
  above a `validator = "0.20"` pin, a `/database/` `cargo add` with every feature dropped, and a
  `/mcp/` stanza naming neither crate `#[mcp]` expands to. Blocks written `workspace = true` are
  not install stanzas and are skipped.
- **`decorator-import`** — a `rust` block that shows **any** `use` line imports every decorator
  it applies. A block with no imports at all reads as a fragment; one that shows them reads as
  pasteable, and 2.0.0 shipped 24 that imported their types and dropped the attribute —
  `use nest_rs::openapi::OpenApiModule;` above a `#[module(...)]`, which is
  `error: cannot find attribute 'module' in this scope` on the first build. `configuration/` held
  four and `http/configuration.mdx` three: the pages a reader opens *to copy a stanza out of*.
  The decorator list is **derived** — every `#[proc_macro_attribute]` under `crates/*-macros/` —
  so the attributes an orchestrator consumes (`#[query]`, `#[get]`, `#[on_module_init]`) are
  never demanded, and a decorator added tomorrow is covered today. A block with `prelude::*` is
  complete by construction and skipped.
- **`layer-impl`** — a type the page **defines** and implements a Layer sub-trait for carries
  `impl Layer for T {}`. There is no blanket impl, and the omission surfaces as an `E0277` naming
  `nest_rs_core::Layer`, which does not say "add a one-line impl". 2.0.0 shipped
  `/fundamentals/middleware/` without it while the guard snippet *on the same page* had it, and
  `/fundamentals/interceptors/` quoted a real framework file with the line stripped out. The
  sub-trait list is **derived** — every `pub trait <T>: Layer` under `crates/` — because a
  hand-written one is wrong the day a sub-trait lands: the first cut listed four and missed
  `GlobalPipe`. Types the page only *names* (the framework's own `AuthnGuard`) are out of scope —
  that impl lives in the framework.
- **`exception-response-error`** — an exception type the page defines and claims via
  `type Exception = E` implements `ResponseError`. An `ExceptionFilter` catches by **downcast off
  an error that is already a `poem::Error`**, so without the impl the handler returning
  `Result<_, E>` does not compile — and the compiler's message (`IntoResult`) names neither the
  trait nor the default status it supplies. The filter *replaces* that status; it does not create
  it. 2.0.0's `/fundamentals/exception-filters/` defined the type and the filter, showed no
  handler, and left the impl behind in the demo file it cited two sections lower.
- **`bare-log`** — a documented `tracing::<level>!` carries at least one structured field, in any
  of the three spellings (`k = v`, `%v`/`?v`, the bare shorthand), whether or not it names a
  `target:` and whether or not rustfmt broke it across lines. `CLAUDE.md`:
  *metadata is mandatory — a bare log is a defect*, since those are the events queried under
  incident. The scaffolds are already held at zero by a unit test over every template
  (`nest-rs-cli/src/templates/mod.rs`); the pages a reader copies from are held to the same bar.
- **`config-table`** — a page publishing a `#[config]` struct's key table lists **every** field,
  and names `staging/production` whenever that struct's `defaults()` branches on the profile. The
  fields are read out of the crate's `config.rs`, not restated. 2.0.0's `/storage/` published five
  of `StorageConfig`'s seven keys under a sentence calling the list exhaustive — the missing
  `ALLOW_HTTP` being the one that decides a boot refusal — and printed the dev branch of a
  profile-split default as *the* default, so a reader preparing a deployment concluded there was
  nothing to pose. Add a page to `CONFIG_TABLES` when it grows such a table.
- **`landing-claim`** — the landing sells the framework on figures, so the figures are read out of
  the repo rather than typed once and left there: `28 capabilities` from the umbrella's feature
  matrix (the derivation the umbrella conformance join runs in Rust), `66 decorators` from the
  decorator index — itself gated against `crates/*-macros/` — `1,800+ tests` from the test
  functions under `crates/`, `120+ pages` from this content tree. Two shapes, on purpose: an
  **exact** count names a set the reader can enumerate elsewhere on the site, so drift is a
  contradiction; a `+` **floor** may lag what the repo holds, but only inside a band, past which
  the page undersells a framework that grew. A missing figure is reported too — dropping the claim
  is dropping the gate, which is how a marketing page starts drifting from the product again.
- **`otel-guard`** — a snippet binding `OpenTelemetry::init` uses the name the crate's own boot
  panic prescribes, read out of `nest-rs-opentelemetry`'s panic text rather than restated. 1.3.0
  corrected the panic to `let _otel =` and left the page's canonical `main` on
  `let _opentelemetry =`, so the reader who tripped the panic was told to write a line the
  example he started from did not contain.

## G. Section tiers — Basics above All options

A section presents **two** lists, not one. **Basics** holds what a reader needs to ship the
section's common case. **All options** holds everything the section also supports:
configuration and tuning, opt-in or specialized capabilities, failure and operational
behaviour, extension seams (writing a driver, an alternative source), and reference tables.
Basics is the shorter of the two — if it holds most of the section, nothing was tiered.

A page that is both — 80% case on top, reference below — is placed by **why a reader opens
it**, never by its content mix. `/http/extractors/` reads as a reference and is Basics: it is
opened to write a handler. `/queue/retries-and-failure/` teaches a contract and is All options:
it is opened once the jobs already run.

The tier is declared **per page, in frontmatter** — `tier: basics` or `tier: all-options` — on
every non-index page of a tiered section. The section `index` declares none: it frames the
split and sits above both groups. Order stays in `sidebar.order`; a tier **partitions** a
section and never restates its order.

**Under five non-index pages a section stays flat**, and a `tier` there is a violation, not a
no-op: two headers over three links cost a reader more than they save. One section is exempt at
any size — `tutorial/` is an ordered path, where a tier boundary mid-sequence would claim
something false.

`docs/src/sidebar.mjs` owns the vocabulary, the threshold and that exemption; `astro.config.mjs`
renders it, `src/content.config.ts` validates the key, and the linter's `tier` rule gates it —
an undeclared page, an unknown tier, or a section that declares only one fails CI. Both sides
fail closed: an undeclared page would drop out of the sidebar, so the **build** stops too.

This is §D made structural. §D budgets one page against drowning the reader; §G budgets the
section, so the long tail is one click away instead of one line away. A T-INDEX page's "In this
section" list (§B) is the same navigation in prose, so it carries the same two tiers.

## Running the linter

```
cd docs
npm run lint:docs              # fails on any violation not in the baseline
npm run lint:docs -- --update-baseline   # re-snapshot known violations (shrinks toward zero)
```

The linter is **baseline-gated**: `docs/scripts/lint-baseline.json` records tolerated violations so
CI fails only on *new* dialect drift. **The baseline is now empty — every rule above gates the
whole corpus, and any violation fails the build.** It only ever shrinks: a new violation is fixed
on the page, never re-snapshotted with `--update-baseline`.

CI runs it on every push, before the build, in `.github/workflows/docs-pages.yml`.
