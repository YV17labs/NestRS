# NestRS docs — style & structure norm

This file is the **single source of truth** for how docs pages are written. It exists because the
corpus was authored across many LLM/human sessions and drifted into dialects. Review holds all
of it — the author, developer or agent, applies these rules; no script does. When in doubt on any
page, apply them.

On conflict about docs prose, this file wins; on conflict about code or naming, `CLAUDE.md` wins.
Where a page and the code disagree, the code wins (§F).

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
  is an excerpt of the Publish workspace, and the author keeps that claim true — the file
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

## F. Facts — the page against the code

A page that reads well and does not run is a defect, and review is what catches it: the author
checks what a page states against the code before it ships. These are the classes that shipped
wrong, each filed by a reader following a page verbatim — check them on sight:

- **A snippet that would not compile.** Every decorator it applies is imported, unless the block
  uses `prelude::*`; a type implementing a Layer sub-trait carries `impl Layer for T {}`; a
  `type Exception = E` implements `ResponseError`; a trait is abridged, never given a method it
  lacks; `Bind<Action, Entity>` takes the action first; a queue is named by its `Queue` type
  on both sides; a handler never `?`s a `CrudService` read, whose `DbErr` is no `ResponseError`.
- **An excerpt that drifted.** A `(from the demo)` title names a file that exists, and every line
  not elided with `// …` is in it, in order.
- **An install line that installs the wrong thing.** A pin is the workspace's `major.minor`; a
  `cargo add` line and the `[dependencies]` block beside it say the same thing; a capability's
  page and its crate README install the umbrella with the feature.
- **A table claiming to be complete.** A `#[config]` key table lists every field, and names the
  profile when the struct's `defaults()` branches on it.
- **A figure or a link.** A count the landing or `/why/` states is one the repo still holds; an
  internal link resolves, and a route that moved gets a `src/redirects.mjs` entry.

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

The group labels live in `astro.config.mjs` and the item labels in frontmatter; review checks
them by reading the built menu and the built breadcrumbs — `dist/**` carries both.
