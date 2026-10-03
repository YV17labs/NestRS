# NestRS docs — style & structure norm

This file is the **single source of truth** for how docs pages are written. It exists because the
corpus was authored across many LLM/human sessions and drifted into dialects. Review holds the
house style below; `docs/scripts/lint-docs.mjs` holds only what a page states that the code, the
demo or the site can contradict (§F), because those are the errors a reader acts on. When in
doubt on any page, apply these rules.

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
moved. `Reference` sits above `Going further`, as an H2 — never a `### Reference` nested inside
the closing block.

Page-specific *content* headings are free. Structural blocks use only the controlled names.

**Banned heading variants** (normalize on sight):

| Banned | Use instead |
|---|---|
| Wiring it up, Wire it into the app, Mount it | Wire it in |
| Where to go next, Next steps, See also, Going deeper | Going further |

Every page states a `description` of at most 160 characters: the one question the page answers.

The normative closing block is **`## Going further`** (the majority convention). Utility and
terminal pages close without one: `404`, `glossary`, `decorators`, the env-var reference, and the
landing, whose every door is already a card.

**It is 2–4 doors wide**: a closing block is where a reader leaves the page, not a second copy of
what the page contained. A section index whose `Going further` had grown to nine links was
listing its own pages under the wrong header — that list is `## In this section` (§ G), and a run
of repository paths is `## Reference`. A bullet naming three sibling transports is one door; a
step in `tutorial/` points at the next step only, so one door is right there.

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
  footgun with consequences. **≤ 3 Asides total per page**, and **every one declares its `type`**
  — an untyped `<Aside>` renders as a note while asserting nothing.
- `<Steps>` for any numbered procedure.
- `<Tabs syncKey=…>` only for genuine alternatives (workspace/standalone).
- **Every fence of file content carries a `title=`** — `rust`, `toml`, `sql`, `graphql`, `ts`,
  `yaml`. Code with no file name is code the reader cannot place. A `bash`
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
  is an excerpt of the Publish workspace, and that claim is what `fence-drift` checks — the file
  exists, and every non-elided line is in it, in order. Add `, abridged` when the excerpt is
  trimmed, and mark each cut with `// …`: `src/posts/entity.rs (from the demo, abridged)`.
  Without the marker a title is an illustration the reader adapts, and it asserts nothing about
  `demo/` — the right title for a simplified variant, a tutorial step, or a shape the demo does
  not use.

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
   words. **Banned words**: *blazing(ly), powerful, seamless(ly), simply, effortless(ly), easy,
   magic(al)*. **No exclamation marks in prose.** The voice is a calm senior engineer showing you
   something that works — never a brochure.
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
takes `posts` / `users` / `orgs` and inventing a name there is the violation this rule names. It
is a review call: an `ItemsService`, an `OrdersController` or a route under `/products` is the
shape an off-canon feature leaks in as.

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

## F. What the linter checks — the page against the facts

A page that reads well and does not run is a defect, and review does not catch it: the page is
plausible on its own, and only the code says otherwise. So the linter checks what a page states
against what the code, the demo and the site hold — a snippet that does not compile or no
longer matches `demo/`, an install line, a version, a figure, a link, a member of a family no
page names. Each rule below was filed against a shipped release by a reader following a page
verbatim. House style (§A–§E, §G) is not here: a page off-style still works, so it is review's.

**Every framework fact here comes from the canon, and the linter derives nothing.** The linter
runs `nest-rs-conformance`'s `canon` binary on start (`cargo run -p nest-rs-conformance --bin
canon` prints it) — capabilities and the crates they activate, decorators, the Layer sub-traits,
the trait surface, the test count, the version requirement, the OTel binding, the queue envelope
keys and `Capability` variants, every `#[config]` struct, the units of work, the span targets,
and the architecture rules' two restated regions. Before it, seven of these checks re-derived
those facts in JavaScript, with regexes; two implementations of one definition drift, and this
pair did — the linter counted 27 capabilities against a landing that correctly said 28. A check
needing a new fact adds a field to the binary, never a regex over `crates/`. Nothing is written
to disk, so no stale copy can pass. What is *content* rather than a derived fact — the `demo/`
files a fence quotes, the READMEs `readme-install` reads — the linter reads directly.

- **`frontmatter`** — every page opens with a frontmatter block: Starlight's schema needs its
  `title`, and the build fails without it.
- **`description`** — a `description` containing ` #` is quoted. YAML ends a plain scalar there,
  so the rest of the sentence silently never reaches the meta tag or the search result.
- **`version-pin`** — a literal `nest-rs* = "X.Y"` (either manifest form) must match
  `[workspace.package] version` in the repo root `Cargo.toml`, which is also what
  `nestrs g resource` writes. Bump the release, bump the pages — or use `workspace = true`,
  which carries no version at all.
- **`bind-order`** — the by-id binder takes its **action marker first**, entity second:
  `Bind<Read, PostEntity>`, and the proof it returns is `Authorized<Read, PostEntity>`. The
  reversed spelling reads plausibly and does not compile, so a page that repeats it teaches the
  wrong rule; ~10 pages shipped it reversed in 1.1.1. Gated rather than trusted.
- **`queue-name`** — a queue is named by its `Queue` **type** on both sides. The consumer's
  `#[process(queue = "audio")]` is a compile error the macro raises by name, and the producer
  pushes with `push(AudioQueue, job, None)`: `push` takes the `#[queue]` marker, so a name
  constant handed to it does not compile, and the string-taking `push_json(name, value, None)`
  is the hatch for a queue this binary does not declare, never the default. The string
  spellings shipped in 1.1.1 across ~10 places, on pages that predated the typed queue; 6.x's
  `push_to::<Q>(job)` is gone, and is named only where a page shows what an upgrade changes.
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
  contradiction; a `+` **floor** is false only once the repo holds fewer, so a floor the repo
  has outgrown is left alone. A missing figure is reported too — a reworded claim the pattern no
  longer finds would otherwise retire the check in silence.
  **A page's surface is its source plus the components it renders**: the landing is MDX importing
  `src/components/*.astro`, and the decorator count is a sentence inside one of them, so the check
  reads both — still `docs/**` exactly.
- **`topology-drift`** — the architecture figure on `/why/` draws the demo's composition, so every
  app, module and edge in its data (`src/topology.mjs`, which the `Topology` component renders)
  is what `demo/apps/*/src/module.rs` imports, in both directions; its module list is the features
  crate's directories, and the collision its caption names is declared in the two files it cites.
  The imports come from the canon's `demo_apps`, read with `syn`, and each is placed by its name —
  `<Module><Edge>Module` an edge from the reserved block's `edges` line, `<Module>Module` a port.
  The figure's first cut, hand-kept, omitted two modules and two of the API's four edges, and
  claimed every binary used the queue: an audit found it, and nothing else would have.
- **`decorator-index`** — `/decorators/` opens by calling itself the index of every decorator the
  framework ships, so every name in the canon's decorator list owes a row. Derived, because a
  hand-kept index is wrong the day a decorator lands and nothing says so.
- **`envelope-drift`** — `/queue/writing-a-driver/` publishes the wire envelope a third-party
  driver has to produce, diffed against the keys the port actually seals into a `nest_rs_queue::Envelope`. A key
  the framework adds and the page omits is a driver that compiles, runs, and drops it across the
  one hop the framework crosses as a *process*.
- **`trait-surface`** — a page may abridge a `pub trait`, it may never invent a method.
  `/fundamentals/exception-filters/` published `Filter` and `ExceptionFilter` with three methods
  each — four names that exist nowhere under `crates/` — then spent an Aside explaining why they
  do not work. A reader who wrote one got `E0407`.
- **`fence-drift`** — a fence whose title says `(from the demo)` is an **excerpt of the file it
  names**: the file exists under `demo/` — any file, a manifest as much as a `.rs` — and every
  non-elided line appears in it, in order. Weaker than § C's byte-for-byte rule on purpose: an
  excerpt may cut (`// …`) and re-indent. It catches every way an excerpt goes stale — a line the
  demo rewrote, a comment the demo does not carry (it carries none), a port the app does not listen
  on, a file that moved, an app the demo never had — and the class it was written for:
  `/security/authentication/` published a `#[module]` inside a file titled `mod.rs`, and
  `/configuration/testing/` a `#[tokio::test]` inside one titled `tests/e2e/main.rs`. A block that
  is not an excerpt drops the marker; that is the escape, and it is § C's generic title.
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
- **`family-mention`** — every member of a family the canon publishes is named on some page, in
  the spelling a reader types: a unit of work (`graphql.operation`, what a dashboard groups on), an
  operator-facing span target (`nest_rs::http`, what `<PREFIX>_LOG` selects on — the DI graph's
  and the dataloader's internal two excepted, in the linter, with the reason), a queue
  `Capability::<Variant>`, and every umbrella capability as a `cargo add nest-rs --features <x>`
  under some page's `## Install`. A family grows a member in Rust, and this makes the docs owe it
  a line the day it exists: two units of work reached 5.1 named on zero of 125 pages. Config env
  keys are deliberately not a family here — a source scan for them is blind to every key read
  through a constant, and a check blind to a quarter of its population is a false guarantee.
- **`readme-install`** — the front door is one crate. A capability crate's own README, its
  crates.io landing page, installs the umbrella with the feature (`cargo add nest-rs --features
  <x>`), and no README tells a reader to `cargo add` a capability sub-crate instead. Both
  halves, because the negative alone passes on an empty corpus.

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

**The split is a reading, not a menu level.** It is drawn by the section index's `## In this
section` list, under a `### Basics` and an `### All options` heading (§B, T-INDEX), and nowhere
else. **The sidebar is two deep and never three**: a group is a section, its items are that
section's pages. Drawn in the menu as well, the split was the third of four levels, and it
charged every reader on the site a level to tell one reader which half of one section a page
sits in — the index is where that reader already is. Order stays in each page's
`sidebar.order`; the index lists each group in that order.

**Under five non-index pages a section stays undivided**: two headers over three links cost a
reader more than they save. One section is exempt at any size — `tutorial/` is an ordered path,
where a tier boundary mid-sequence would claim something false.

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
npm run lint:docs   # the gate
npm test            # the linter joined against itself
```

**There is no baseline.** Every rule is a fact a page contradicts, so a violation is fixed on the
page — or the rule is wrong, and the rule is fixed. A list of tolerated violations would be a list
of pages known to mislead their reader. A new rule lands with the pages it finds already fixed.

A clean run only means something if the walk read the corpus, so **below 100 pages the gate fails**
instead of reporting success: rename a section directory and its pages leave the walk, every rule
over them stops running, and the build would go greener.

`npm test` is the other half. `scripts/lint.test.mjs` joins the rules against themselves: every
member of `RULES` owes a **fixture that makes it fire** and a **§F entry above**, and no violation
may name a rule outside the set. A rule added without a fixture fails, a rule weakened until it
matches nothing fails, and a §F entry for a rule that does not exist fails. A fixture proves a
rule triggers, never that its judgement is right; that question is `/audit`'s.

CI runs the gate before the build, in `.github/workflows/docs-pages.yml`, on pushes to `main`
that touch `docs/**` or anything the canon and the quoted sources are read from — `crates/**`,
`demo/**`, the root manifest and lockfile, the root README — so a framework change that falsifies
a page trips the job on the commit that caused it.
