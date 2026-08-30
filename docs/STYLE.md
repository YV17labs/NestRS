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

`Install` → `Wire it in` → `Run it` → *(page-specific content sections)* → `Configuration` →
`Limits` → `What fails if you get it wrong` → `Reference` → `Going further`

Recorded because it was written the other way round and every page disagreed: the order used to
put `Run it` above `Wire it in`, and not one of the pages carrying both followed it — you cannot
run what is not yet mounted. The pages were right and the sentence was wrong, so the sentence
moved. `Reference` before `Going further` is the half that was *not* followed — sixteen pages
nested a `### Reference` inside the closing block — and that one is now the `reference-order`
rule rather than a convention.

Page-specific *content* headings are free. Structural blocks use only the controlled names.

**Banned heading variants** (normalize on sight):

| Banned | Use instead |
|---|---|
| Wiring it up, Wire it into the app, Mount it | Wire it in |
| Where to go next, Next steps, See also, Going deeper | Going further |

A heading from the left column is the `heading` rule. Frontmatter is `frontmatter` (present at
all) and `description` (present, ≤ 160 characters, no unquoted `#` — YAML truncates there and the
sidebar shows half a sentence).

The normative closing block is **`## Going further`** (`going-further`; the majority convention). Utility/terminal
pages are exempt (see the linter's exempt list): `404`, `glossary`, `decorators`, env-var
reference.

**It is 2–4 doors wide**, and the same rule counts them: a closing block is where a reader leaves
the page, not a second copy of what the page contained. A section index whose `Going further` had
grown to nine links was listing its own pages under the wrong header — that list is `## In this
section` (§ G), and a run of repository paths is `## Reference`. A bullet naming three sibling
transports is one door; a step in `tutorial/` points at the next step only, so one door is right
there and the rule leaves it alone.

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
  footgun with consequences. **≤ 3 Asides total per page** (`asides`), and **every one declares
  its `type`** (`aside-type`) — an untyped `<Aside>` renders as a note while asserting nothing,
  which is what twenty-six of them did, several being real cautions.
- `<Steps>` for any numbered procedure.
- `<Tabs syncKey=…>` only for genuine alternatives (workspace/standalone).
- **Every fence of file content carries a `title=`** — `rust`, `toml`, `sql`, `graphql`, `ts`,
  `yaml` (`fence-untitled`). Code with no file name is code the reader cannot place. A `bash`
  block is a command, a `json`/`http`/`text` block is a payload or an output, `mermaid` is a
  picture: none is a file, so none takes one.

  The title is **one of three shapes, and there is no fourth**:

  | Shape | When | Example |
  |---|---|---|
  | a **file path** | the reader writes this into a file | `src/posts/http/controller.rs` |
  | a **framework path** | the fence shows the framework's own surface, or its use | `nest_rs::pipes::Pipe` |
  | a short **lowercase label** | the fence spans several files, or is not code you write | `from the macro expansion` |

  A path title obeys the architecture rules like any other path — `src/users/graphql/resolver.rs`,
  never `src/users/resolver.rs` — because a title is the one place a page states the layout it is
  teaching.

  **Every path is workspace-shaped. There is no standalone path in the docs.** `nestrs new`
  produces a workspace by default — `crates/features/` plus thin `apps/*` — and that is the
  layout the docs teach, so every title sits under it: feature code at
  `crates/features/src/<module>/<role>.rs`, composition and bootstrap at `apps/<app>/src/…`,
  end-to-end suites at `apps/<app>/tests/e2e/…`, a driver the reader writes at
  `crates/<vendor>/src/…`. `--standalone` exists and its `src/…` shape is real, but a page that
  shows both teaches neither: the reader cannot tell a different layout from a different file.
  One shape, and it is the one we recommend.

  It binds the code as well as the caption. An `apps/…` fence reaches a feature through the
  `features` crate — `use features::blog::BlogService;` — and never `use crate::`, which is what
  the same file would say in the standalone layout and what three pages were still showing.

  **Provenance is a word, not a prefix: `(from the demo)`.** A title carrying it claims the block
  is an excerpt of the Publish workspace, and that claim is what `fence-drift` and `fence-title`
  check — the file exists at all (the canon indexes every `.rs` under `demo/`, so a marker on a
  path it lacks is provably false), every non-elided line in the real file, in order, no comment
  the demo does not carry, no port it does not listen on. Add `, abridged` when the excerpt is trimmed:
  `src/posts/entity.rs (from the demo, abridged)`. Without the marker a title is an illustration
  the reader adapts, and it asserts nothing about `demo/`.

  Recorded because it was decided against the obvious alternative: the prefix used to *be* the
  claim, so one string meant two things — the reader's layout and our provenance — and a page
  showed the same file at two roots with nothing saying why (21 such conflicts, on 20 pages).
  Promoting every illustration to the long prefix so the site read one way reported **135
  violations across 47 pages**, which is the measurement that settled it: the prefix was carrying
  the provenance, and only the provenance needed saying.

  GitHub URLs still use the real repo paths (`demo/crates/features/…`) — a URL has to resolve.
- Terminal transcripts: `$`-prefixed input lines, trimmed output (≤ ~8 meaningful lines), no
  fabricated sequencing (a log line never appears before the command that causes it).
- One `Piped` destructuring style, one boot-log format across pages.

## D. The anti-drowning charter (simplicity is a budget)

1. **Page budgets.** A reference page: ≤ ~250–300 lines, answers **one question** (the one its
   frontmatter description states). A tutorial page: ≤ ~250 lines, ends on a runnable checkpoint.
   Per page: ≤ 3 Asides; the first screen is one working snippet (≤ ~15 lines) with **no Aside
   above it**.

   **A caution is placed by what it warns about, not by where the page ends.** One anchored to the
   snippet above it stays there — that is the moment the reader can act on it, and moving it to a
   list at the bottom is how a footgun becomes a footnote. `Limits` collects the constraints that
   have **no single anchor**: what the feature will not do, when to leave it off, what a proxy in
   front of it changes. The earlier wording said every scattered caution consolidates, and the
   corpus disagreed with it in the right direction — fifty-one cautions, nearly all of them
   correctly anchored — so the rule now says what the good pages do. What stays capped is the
   *count*: three Asides is the budget, and a page needing more is a page to split.
2. **Evidence placement.** Proof follows the promise it proves. Never a failure demo before the
   reader's first success. Boot/compile errors live under `What fails if you get it wrong` *after*
   the 80% case. Verbatim outputs are real (run it once, paste it), trimmed to ≤ ~8 lines. **Each
   evidence artifact appears once site-wide** — every other page links to it.
3. **Competitor mentions.** Named competitors (NestJS, BullMQ, Socket.IO, Sidekiq, Hasura…) appear
   **only** on the landing, `why.mdx`, and the comparison page. Reference pages sell by
   demonstration.
4. **Prose style charter.** Second person, present tense, active voice. Average sentence ≤ ~22
   words. **Banned words** (`banned-word`): *blazing(ly), powerful, seamless(ly), simply,
   effortless(ly), easy, magic(al)*. **No exclamation marks in prose** (`exclamation`). The voice is a calm senior
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
| WebSockets | `chat` / the `notifications` ws edge (`demo/crates/features`, served by `demo/apps/live`) |
| Queue + Schedule | `audio` / `TranscodeCommand` (`demo/apps/worker`) |
| Events | `PostPublishedEvent` (notifications listener) |
| MCP | `weather` (+ `hello` tool) (`demo/apps/assistant`) |
| OpenAPI, Health, Rate limiting, OTel, Testing | the `api` app over `users`/`posts` |
| Storage | the `audio` slice's uploads (`demo/crates/features/src/audio`) |

## F. Code truth — the checks the prose rules can't see

Style is half the job; a page that reads well and does not run is still a defect. Each of these
was filed against a shipped release by a reader following a page verbatim, so the linter now
greps for them:

**Every check here reads `docs/canon.json`, and the linter derives nothing.** That file is
generated by `nest-rs-conformance`'s `canon` join — capabilities, decorators, the Layer
sub-traits, the trait surface, the test count, the version requirement, the OTel binding, the
queue envelope keys, every `#[config]` struct, the architecture rules' two restated regions, and
every demo file with the port it pins. Before it, seven of these checks opened `crates/**` and
`demo/**` and re-derived those facts in JavaScript, with regexes; two implementations of one
definition drift, and this pair did — the linter counted 27 capabilities against a landing that
correctly said 28. A check needing a new fact adds a field to the join, never a `readFileSync`
outside `docs/`.

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
  nothing to pose. Which *page* publishes a table is a docs-side fact and stays in
  `CONFIG_TABLES`; what the struct holds comes from the canon. Add a page there when it grows
  such a table.
- **`landing-claim`** — the site sells the framework on figures, so the figures are read out of
  the repo rather than typed once and left there. Four of them, on the two pages that make the
  claim: `/` carries the **capability count** (from the canon) and the **decorator count** (from
  the decorator index, itself gated against the canon's decorator list); `/why/` carries the
  **test floor** (from the canon) and the **page count** (from this content tree), because that is
  the page arguing the framework holds its shape, and the redesigned splash states neither — a
  figure with no home on a page is a gate with no subject, and the repair is to gate it where the
  claim is made rather than to delete the check. **A capability is a feature a developer can name
  in `--features`**, not a crate; the two number the same today and did not before `seaorm` grew a
  second `dep:`, which is the drift that produced this paragraph. Two shapes, on purpose: an
  **exact** count names a set the reader can enumerate elsewhere on the site, so drift is a
  contradiction; a `+` **floor** may lag what the repo holds, but only inside a band, past which
  the page undersells a framework that grew. A missing figure is reported too — dropping the claim
  is dropping the gate, which is how a marketing page starts drifting from the product again.
  **A page's surface is its source plus the components it renders**: the landing is MDX importing
  `src/components/*.astro`, and the decorator count is a sentence inside one of them, so the check
  reads both — still `docs/**` exactly.
- **`decorator-index`** — `/decorators/` opens by calling itself the index of every decorator the
  framework ships, so every name in the canon's decorator list owes a row. Derived, because a
  hand-kept index is wrong the day a decorator lands and nothing says so.
- **`envelope-drift`** — `/queue/writing-a-driver/` publishes the wire envelope a third-party
  driver has to produce, diffed against the keys `nest_rs_queue::envelope` actually seals. A key
  the framework adds and the page omits is a driver that compiles, runs, and drops it across the
  one hop the framework crosses as a *process*.
- **`trait-surface`** — a page may abridge a `pub trait`, it may never invent a method.
  `/fundamentals/exception-filters/` published `Filter` and `ExceptionFilter` with three methods
  each — four names that exist nowhere under `crates/` — then spent an Aside explaining why they
  do not work. A reader who wrote one got `E0407`.
- **`for-root-form`** — the seam takes `impl Into<Option<C>>`, so a snippet writing
  `for_root(Some(cfg))` teaches a spelling the signature does not need.
- **`fence-title`** — a fence titled with a real `demo/` file may not contradict it. Two exact
  probes rather than the byte-for-byte rule of §C: a comment (the demo workspace carries none, so
  quoting one publishes code the repo forbids writing) and a `port:` disagreeing with the app's.
  The strict form would report 134 pages at once and the signal would be gone; the narrowing is
  deliberate and this sentence is where it is stated.
- **`fence-untitled`** — a fence whose language is the *content of a file* — `rust`, `toml`,
  `sql`, `graphql`, `ts`, `yaml` — carries a `title=`. A reader who cannot see where a snippet
  goes cannot use it, and 244 fences across 71 pages said nothing. The complement is the
  argument: a `bash` block is a command, a `json`/`http`/`text` block is a payload or an output,
  `mermaid` is a picture — none of them is a file, and none owes a title. See § C for the three
  shapes a title may take.
- **`test-layout`** — a test target is a directory (`tests/<suite>/main.rs`), so a page
  prescribing a flat `tests/<x>.rs` in a fence title or a table cell teaches a suite that escapes
  the `binary(e2e)` gate. Scoped to prescriptive lines, because naming the flat form is exactly
  how `/testing/e2e/` refuses it.
- **`fence-drift`** — a fence titled with a real `demo/` file is an **excerpt of that file**:
  every non-elided line appears in it, in order. Weaker than § C's byte-for-byte rule on purpose
  — most fences are honest excerpts written before the `(abridged)` convention, and the strict
  form reports 134 pages at once, which is a signal nobody reads. What it catches is the class
  byte-for-byte was written for and nothing enforced: `/security/authentication/` published a
  `#[module]` inside a file titled `mod.rs`, which the architecture rules the CLI generates into
  every scaffolded project forbid, and `/configuration/testing/` published a `#[tokio::test]`
  inside one titled `tests/e2e/main.rs`, which the locked test-layout norm forbids. Both sat on
  pages a reader opens first. A snippet that is *not* an excerpt gets a title that does not name
  a repo file — that is what § C means by a generic title, and it is the escape.
  **102 pre-existing drifts are baselined**; the list only shrinks, and a 103rd fails the build.
- **`link`** — every internal link resolves to a page the site serves (or a declared redirect),
  and every `#anchor` to a heading on the page it lands on. Nothing checked this: a probe page
  linking a route that does not exist builds clean, exits 0, and ships the dead href — the only
  validated targets on the whole site were the ~20 sidebar `slug:` entries, against 969 in-page
  links. Starlight's own answer is a plugin; a link check reads the page corpus and nothing else,
  which makes it a rule rather than a dependency. Anchor ids follow GitHub's algorithm, and the
  implementation is deliberate: `github-slugger` last published 2023-09-15, outside the
  twelve-month freshness bar `CLAUDE.md` sets, so it is flagged and not adopted — the algorithm is
  written out and verified against the built site, all 933 anchors agreeing in both directions.
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

**The split is a reading, not a menu level.** It is drawn by the section index's "In this
section" list (§B) and nowhere else. **The sidebar is two deep and never three**: a group is a
section, its items are that section's pages. Drawn in the menu as well, the split was the third
of four levels, and it charged every reader on the site a level to tell one reader which half of
one section a page sits in — the index is where that reader already is.

The tier is declared **per page, in frontmatter** — `tier: basics` or `tier: all-options` — on
every non-index page of a tiered section. The section `index` declares none: it frames the
split and sits above both groups. Order stays in `sidebar.order`; a tier **partitions** a
section and never restates its order.

**Under five non-index pages a section stays undivided**, and a `tier` there is a violation, not
a no-op: two headers over three links cost a reader more than they save. One section is exempt at
any size — `tutorial/` is an ordered path, where a tier boundary mid-sequence would claim
something false.

`docs/src/sidebar.mjs` owns the vocabulary, the threshold and that exemption;
`src/content.config.ts` validates the key, and the linter's `tier` rule gates it — an unknown
tier fails the **build**, and an undeclared page or a section declaring only one fails CI.

**The other half is gated too, and it is the half that was missing: `section-index`.** A tiered
section's index must carry `## In this section` with a `### Basics` and an `### All options`
under it. Eight of the ten tiered sections shipped without that list — `tier:` in the frontmatter
of some fifty pages, validated by the schema, gated by the linter, and rendered to the reader
nowhere at all. A tier declared and never drawn is not a weaker split; it is no split, plus the
cost of maintaining one. `/http/` and `/graphql/` are the two that had it, and they are the
shape to copy.

This is §D made structural. §D budgets one page against drowning the reader; §G budgets the
section, so the page that frames it says which half a reader came for.

## H. The menu — two levels, everything shown, no name said twice

**The sidebar lists every section and every page in it, always.** Nothing unfolds: a group is a
section, its items are that section's pages, and the reader sees the whole map rather than the
branch they happen to be standing on.

That has one consequence and it is the whole of this section: **a label is read against the
entire column, not against its own header.** Two rules follow, and both are absolute.

- **No item repeats its group's label.** A section's index would otherwise say the section's
  name directly under the section's name — `HTTP › HTTP`, nine times over. When the group is
  named after the section, its index is labelled **`Overview`**, which is what the design draws
  and what the reader is actually being offered. Where the group is *broader* than its index the
  index keeps its own name, because there it carries information the header does not:
  `Data › Database`, `Background work › Queue`, `Operations › OpenTelemetry`.
- **No two items share a label.** A short label was only ever legible because the rest of the
  menu was hidden. `Configuration` under HTTP, GraphQL and MCP — with a top-level
  `Configuration` group in the same column — is three pages and a section wearing one word. The
  page's own qualified `title` is the fix: `HTTP configuration`, `GraphQL errors`,
  `WebSocket guards`. A short `sidebar.label` is for a name nothing else in the menu claims.

**A page has one navigation name, and the breadcrumb uses it too.** The trail is read out of the
hydrated sidebar, so its last segment is the page's menu label rather than its `title`. Those
differ on every section index — the title is the section's name, the label is `Overview` — and
taking the title printed the section twice in a row: `HTTP / HTTP`, which is the same defect the
menu had, one component further along. The `title` stays what the `<h1>` and the search index
show.

Not gated: the group labels live in `astro.config.mjs` and the item labels in frontmatter, and
joining the two would mean the linter importing the Astro config. Checked by reading the built
menu and the built breadcrumbs — `dist/**` carries both — and by this section.

## Running the linter

```
cd docs
npm run lint:docs                    # the gate
npm test                             # the linter joined against itself
npm run lint:docs -- --land <rule>   # land a new rule on the corpus it inherits
```

The linter is **baseline-gated**: `docs/scripts/lint-baseline.json` records the violations a rule
inherited on the day it landed, so CI fails only on *new* dialect drift. The contract runs **both
directions**, enforced rather than promised — a violation not in the baseline fails, *and* a
baseline line naming a violation since fixed fails, so deleting that line is part of fixing the
page instead of a chore nobody is prompted to do. The list only ever shrinks. Every rule above
gates the whole corpus at zero except `fence-drift`, whose entry says why it landed on a corpus
written before it.

**`--update-baseline` is gone, and its removal is the rule.** It re-snapshotted every current
violation, the code-truth ones of §F included — so the remedy the failure message printed could
turn a proven-false claim about the framework into a permanent exemption, in a file nobody
re-reads, and it was offered to whoever held the red build, who is routinely not the author of
the break. `--land` is the narrow replacement: it names **one** rule, refuses a name `RULES` does
not hold, writes that rule's current violations and then **fails**, so a landing is a reviewed
commit rather than a silent green. Anything else is fixed on the page; a line that genuinely
belongs in the baseline is added by hand, where a reviewer sees it.

A clean run only means something if the walk read the corpus, so **below 100 pages the gate fails**
instead of reporting success. That is the mirror of the baseline: a baseline catches a corpus that
grew a violation, and nothing caught a corpus that *shrank* — rename a section directory and its
pages leave the walk, every rule over them stops running, and the build goes greener.

`npm test` is the other half. `scripts/lint.test.mjs` joins the rules against themselves: every
member of `RULES` owes a **fixture that makes it fire** and a **§F entry above**, and no violation
may name a rule outside the set. A rule added without a fixture fails, a rule weakened until it
matches nothing fails, and a §F entry for a rule that does not exist fails. Before that join,
thirty rules had no proof they still fired — neutralise any regex and the gate went greener. A
fixture proves a rule triggers, never that its judgement is right; that question is `/audit`'s.

CI runs the gate on pushes to `main` that touch `docs/**`, before the build, in
`.github/workflows/docs-pages.yml`. That filter is the job's whole input set, exactly — the
linter opens no file outside `docs/`, which is what `docs/canon.json` bought. A framework change
that moves a documented fact regenerates the canon, so it lands a `docs/**` diff and trips this
job on the commit that caused it.
