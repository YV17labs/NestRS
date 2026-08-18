#!/usr/bin/env node
// Docs prose/structure linter — enforces docs/STYLE.md.
//
// Baseline-gated: docs/scripts/lint-baseline.json records currently-tolerated violations so CI
// fails only on NEW dialect drift. As pages reach conformance the baseline shrinks; when empty,
// the linter gates the whole corpus.
//
//   node scripts/lint-docs.mjs                    # fail on any violation not in the baseline
//   node scripts/lint-docs.mjs --update-baseline  # re-snapshot known violations
//
// Checks (see STYLE.md): controlled H2 vocabulary, banned prose words + exclamation marks,
// frontmatter description present / ≤160 / no unquoted '#', closing "## Going further",
// ≤3 Asides per page, example-canon ban list, the Basics / All options tier split.
// Plus the code-truth checks the prose rules can't see — `version-pin`, `unauthed-curl`,
// `crud-error`, `bind-order`, `queue-name`, `install-stanza`, `otel-guard`, `decorator-import`,
// `layer-impl`, `exception-response-error`, `bare-log`, `config-table` — each documented on its
// constant below and filed as a shipped defect first.

import { readFileSync, writeFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';
import {
  CONTENT_ROOT, TIERS, TIER_LABELS, TIER_THRESHOLD, UNTIERED_SECTIONS, sections,
} from '../src/sidebar.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS_ROOT = join(HERE, '..');
// The repo the docs live in, spelled once — every source-derived check roots here.
const REPO_ROOT = join(DOCS_ROOT, '..');
// The same root the sidebar reads, so the two never disagree about what a page is.
const CONTENT = CONTENT_ROOT;
// Every page, walked once. Two consumers — the per-file lint pass, and the
// landing's `N+ pages` floor, which is *checked against this number*: a second
// walk would be a second answer to the question the claim is gated on.
const PAGES = walk(CONTENT).sort();
const BASELINE = join(HERE, 'lint-baseline.json');

// Pages exempt from the closing "Going further" requirement (utility/terminal pages).
const GOING_FURTHER_EXEMPT = new Set([
  '404.md',
  'glossary.mdx',
  'decorators.mdx',
  'configuration/env-reference.mdx', // env-var reference (step 10)
]);

const BANNED_HEADINGS = [
  'wiring it up', 'wire it into the app', 'where to go next',
  'next steps', 'see also', 'going deeper',
];

const BANNED_WORDS = [
  'blazing', 'blazingly', 'powerful', 'seamless', 'seamlessly',
  'simply', 'effortless', 'effortlessly', 'easy', 'magic', 'magical',
];

const CANON_BANLIST = [
  'ItemsService', 'ProductEntity', 'artworks', 'file_assets', 'Ledger',
  // whole-word 'items'/'products' as a feature name are context-heavy; the identifiers above
  // are the reliable signal.
];

/// Off-canon features leak in as *variants* of the banned identifiers —
/// `ItemsController`, `ItemsResolver`, a `path = "/items"`, a `src/items/`
/// snippet title. A word-list can't keep up; these shapes can.
/// `items` as plain English ("items reachable from the root") is untouched.
const CANON_SHAPES = [
  [/\b(?:Item|Product|Order)s?(?:Controller|Resolver|Service|Entity|Module|Gateway|Processor)\b/,
    'off-canon feature type'],
  [/(?:path|title)\s*=\s*"[^"]*\/(?:items|products|orders)\b/, 'off-canon feature path'],
  [/#\[(?:get|post|patch|put|delete)\("\/(?:items|products|orders)\b/, 'off-canon feature route'],
  // The residue the type and path shapes both miss: an entity keeps the banned
  // table under a canon `#[expose(name = "Post")]`, and a GraphQL `path` or a
  // `data` key still spells the field the old example resolved.
  [/table_name\s*=\s*"\w*(?:item|product|order)s?\w*"/, 'off-canon table'],
  [/"(?:path|data)":\s*[[{]\s*"(?:item|product|order)s?"/, 'off-canon wire field'],
];

/// A file in the repo the docs live in, by repo-relative path. Every code-truth
/// check that derives its rule from the framework rather than restating it goes
/// through here, so the tree layout is spelled once.
function frameworkSource(rel) {
  return readFileSync(join(REPO_ROOT, ...rel.split('/')), 'utf8');
}

/// The architecture rules, as the CLI embeds them into every generated
/// project's `AGENTS.md` (and as `.claude/rules/` symlinks them).
const ARCHITECTURE_CANON = 'crates/nest-rs-cli/src/templates/architecture.md';

/// Pages that restate a file the framework ships, keyed by rel like
/// [`CONFIG_TABLES`]. Registering here is what makes the mirror *checked*, and
/// the run asserts every entry was actually visited — rename or move the page
/// and the build fails rather than the gate quietly ceasing to run.
/// The value carries the rule the violations file under, so a second mirror does
/// not have to borrow the first's name.
const MIRRORED_PAGES = new Map([
  ['architecture.mdx', { rule: 'architecture-drift', check: (src) => architectureDrift(src) }],
  ['decorators.mdx', { rule: 'decorator-index', check: (src) => decoratorIndexDrift(src) }],
  ['index.mdx', { rule: 'landing-claim', check: (src) => landingClaims(src) }],
  ['queue/writing-a-driver.mdx', { rule: 'envelope-drift', check: (src) => envelopeDrift(src) }],
]);
const MIRRORS_SEEN = new Set();

/// Backticked file and folder roles — ``service.rs``, ``http/controller.rs``,
/// ``services/`` — taken from **table rows only**. Row *labels* are free to
/// read differently on the docs page than in the shipped file, and so is every
/// sentence around the table: the page teaches in its own voice, and only the
/// roles it tabulates have to agree.
function roleTokens(src) {
  const rows = src.split('\n').filter((l) => l.startsWith('|'));
  return new Set(
    [...rows.join('\n').matchAll(/`([a-z_]+\/)?[a-z_]+\.rs`|`[a-z_]+\/`/g)].map((m) => m[0]),
  );
}

/// The fenced block listing the words a module may not be named after. Goes
/// through `fencedBlocks` rather than its own fence regex, so it cannot skip
/// past an intervening heading and compare an unrelated block.
const RESERVED_SECTIONS = new Set(['Reserved vocabulary', 'What a folder may not be called']);
function reservedWords(src) {
  const block = fencedBlocks(src).find((b) => RESERVED_SECTIONS.has(b.section));
  return block ? new Set(block.body.split(/\s+/).filter(Boolean)) : null;
}

/// Both directions of a set comparison, as the messages a reader acts on.
function setDrift(canon, page, what) {
  return [
    ...[...canon].filter((x) => !page.has(x)).map((x) => `${what} missing from the page: ${x}`),
    ...[...page].filter((x) => !canon.has(x)).map((x) => `${what} on the page, not in the rules: ${x}`),
  ];
}

/// `/queue/writing-a-driver/` publishes the wire envelope a third-party driver
/// has to produce, so its JSON block is diffed against the keys
/// `nest_rs_queue::envelope` actually seals rather than trusted. A key the
/// framework adds and the page omits is a driver that compiles, runs, and drops
/// that key across the one hop the framework crosses as a *process*.
const ENVELOPE_CANON = 'crates/nest-rs-queue/src/envelope.rs';
function envelopeDrift(src) {
  const canon = frameworkSource(ENVELOPE_CANON);
  const keys = [...canon.matchAll(/^const [A-Z_]+: &str = "([a-z_]+)";$/gm)].map((m) => m[1]);
  if (keys.length === 0) {
    throw new Error(`no envelope key constants in ${ENVELOPE_CANON} — teach \`envelopeDrift\``);
  }
  return keys
    .filter((key) => !src.includes(`"${key}"`))
    .map((key) => `${key} is sealed into the wire envelope and the page's shape omits it — `
      + 'a driver written from this page would drop it');
}

/// `/decorators/` opens by calling itself "the index of every decorator the
/// framework ships", so a decorator with no row makes that opening false. The
/// list is derived from the `*-macros` crate roots rather than restated, and the
/// page is registered in [`MIRRORED_PAGES`] so renaming it throws instead of
/// retiring the check in silence.
function decoratorIndexDrift(src) {
  return DECORATORS.filter((name) => !src.includes(`\`#[${name}]\``)).map(
    (name) => `#[${name}] is a shipped decorator with no row — the page opens by calling `
      + 'itself the index of every one',
  );
}

/// Every capability the umbrella's feature matrix offers — a feature whose value
/// activates a `dep:nest-rs-<x>`, which is what separates the capabilities from
/// `default` and `full`. The same derivation the umbrella conformance join runs
/// in Rust, so the landing's count and the suite's floor cannot disagree.
function umbrellaCapabilities() {
  const manifest = frameworkSource('crates/nest-rs/Cargo.toml');
  const start = manifest.indexOf('\n[features]');
  const rest = start === -1 ? '' : manifest.slice(start + 1);
  const next = rest.slice(1).search(/\n\[[a-z]/);
  const body = next === -1 ? rest : rest.slice(0, next + 1);
  const count = [...body.matchAll(/^[\w-]+\s*=\s*\[([\s\S]*?)\]/gm)].filter(([, value]) =>
    /dep:nest-rs-/.test(value),
  ).length;
  // Fail closed: a matrix that stopped parsing would silently agree with any number.
  if (!count) {
    throw new Error('no capability feature found in crates/nest-rs/Cargo.toml — teach '
      + '`umbrellaCapabilities` where the feature matrix moved, do not delete the check');
  }
  return count;
}

/// Every decorator `/decorators/` tabulates, counted the way a reader would: the
/// distinct `#[name]` tokens in its first column. That page is itself gated
/// against `crates/*-macros/` by [`decoratorIndexDrift`], so a figure checked
/// against it is checked against the source one hop away — and the inert
/// attributes an orchestrator reads (`#[get]`, `#[public]`) count here exactly
/// because the page's inclusion rule is "everything you write".
function documentedDecorators() {
  const src = readFileSync(join(CONTENT, 'decorators.mdx'), 'utf8');
  const names = new Set();
  for (const row of src.split('\n').filter((l) => l.startsWith('| `#['))) {
    for (const m of row.slice(1, row.indexOf('|', 1)).matchAll(/#\[(\w+)/g)) names.add(m[1]);
  }
  if (!names.size) {
    throw new Error('no decorator rows in decorators.mdx — teach `documentedDecorators` where '
      + 'the index moved, do not delete the check');
  }
  return names.size;
}

/// The landing sells the framework on figures, so the figures are read out of
/// the repo rather than typed once and left. Four claims, four sources: the
/// umbrella's feature matrix, the decorator index, the test functions under
/// `crates/`, and this content tree.
///
/// Two shapes, and the difference is deliberate. An **exact** claim (`28
/// capabilities`) names a set the reader can enumerate on another page of this
/// site, so any drift is a contradiction. A **floor** (`1,800+ tests`) may lag
/// what the repo holds — that is what the `+` says — but only within a band:
/// past it the page undersells a framework that grew, which is the same defect
/// pointing the other way.
const CLAIM_BAND = new Map([['tests', 600], ['pages', 30]]);
function landingClaims(src) {
  const out = [];
  const claim = (re) => {
    const m = src.match(re);
    return m ? Number(m[1].replace(/,/g, '')) : null;
  };
  const exact = [
    ['capabilities', claim(/\*\*(\d[\d,]*) capabilities\*\*/), umbrellaCapabilities()],
    ['decorators', claim(/\*\*(\d[\d,]*) decorators\*\*/), documentedDecorators()],
  ];
  for (const [what, claimed, actual] of exact) {
    if (claimed === null) {
      out.push(`no \`**N ${what}**\` claim on the landing — the figure is gated against the `
        + `repo, so removing it removes the gate; say what the repo holds (${actual})`);
    } else if (claimed !== actual) {
      out.push(`the landing claims ${claimed} ${what}, the repo holds ${actual}`);
    }
  }
  const floors = [
    ['tests', claim(/\*\*(\d[\d,]*)\+ tests\*\*/), TEST_COUNT],
    ['pages', claim(/(\d[\d,]*)\+ pages/), PAGES.length],
  ];
  for (const [what, claimed, actual] of floors) {
    const band = CLAIM_BAND.get(what);
    if (claimed === null) {
      out.push(`no \`N+ ${what}\` claim on the landing — the figure is gated against the repo, `
        + `so removing it removes the gate; say what the repo holds (${actual})`);
    } else if (claimed > actual) {
      out.push(`the landing claims ${claimed}+ ${what}, the repo holds ${actual}`);
    } else if (actual - claimed > band) {
      out.push(`the landing claims ${claimed}+ ${what} and the repo holds ${actual} — past `
        + `${band} the floor undersells; raise it`);
    }
  }
  return out;
}

/// `/architecture/` deliberately restates the role table and the reserved
/// vocabulary: they are what a reader opens the page for, and sending them to a
/// file in the repo would be a worse page. Restating is fine, *drifting* is not
/// — and only a check catches that, because both sides read plausibly on their
/// own.
///
/// Fails closed on both sides. A canon that stops parsing throws (the rules
/// moved and this check is now vacuous); a page that stops parsing reports the
/// missing section **once**, rather than one line per token it can no longer
/// find.
function architectureDrift(src) {
  const canon = frameworkSource(ARCHITECTURE_CANON);
  const out = [];

  const canonRoles = roleTokens(canon);
  if (canonRoles.size === 0) {
    throw new Error(`no role table in ${ARCHITECTURE_CANON} — teach \`architectureDrift\``);
  }
  const pageRoles = roleTokens(src);
  if (pageRoles.size === 0) out.push('role table missing from the page');
  else out.push(...setDrift(canonRoles, pageRoles, 'role'));

  const canonWords = reservedWords(canon);
  if (!canonWords) {
    throw new Error(`no reserved-vocabulary block in ${ARCHITECTURE_CANON}`);
  }
  const pageWords = reservedWords(src);
  if (!pageWords) out.push('reserved-vocabulary block missing from the page');
  else out.push(...setDrift(canonWords, pageWords, 'reserved word'));

  return out;
}

/// `major.minor` of the framework the repo currently builds — what every
/// documented `nest-rs*` pin has to say, and what `nestrs g resource` writes
/// into a generated manifest.
function workspaceVersionReq() {
  const m = frameworkSource('Cargo.toml')
    .match(/^\[workspace\.package\]$[\s\S]*?^version\s*=\s*"(\d+\.\d+)\./m);
  if (!m) throw new Error('no [workspace.package] version in the repo root Cargo.toml');
  return m[1];
}

const VERSION_REQ = workspaceVersionReq();

/// A `nest-rs*` dependency line pinning a literal version, in either manifest
/// form: `nest-rs-authz = { version = "1.1", … }` and `nest-rs-resource = "1.1"`.
/// `workspace = true` lines carry no version and never match.
const NEST_RS_PIN =
  /\bnest-rs[a-z0-9-]*\s*=\s*(?:\{[^}\n]*?version\s*=\s*)?"([^"]+)"/g;

/// REST route roots the Publish canon serves behind `AuthnGuard` + `AuthzGuard`.
/// A `curl` a reader can paste has to carry a bearer against one of these — the
/// guards run before validation, before the pipe, before the handler, so a
/// documented `400`/`200` reached without a token is a response the reader
/// never sees.
///
/// `/graphql` is deliberately absent: one endpoint, per-operation posture, so
/// the path cannot tell you whether a bearer is required (the reference pages
/// query `#[public]` toys through it).
const GUARDED_ROUTE_ROOTS = new Set([
  'posts', 'users', 'orgs', 'notifications', 'media', 'audio',
]);

/// `CrudService`'s read half returns `Result<_, DbErr>`, and `DbErr` has no
/// `ResponseError` impl — so `?` on one of these inside a handler does not
/// compile. Named individually: `create`/`update`/`delete` are routinely
/// overridden by a service method that *does* return `ServiceError`, and those
/// snippets are correct.
const UNMAPPED_CRUD_READ = /\.(?:list\(\)|page\(|access\()[^;]*?\.await\s*\?/;

/// `Bind` and `bind` take the **action marker first**, the service second
/// (`nest-rs-seaorm/src/http/bind.rs`, `…/src/graphql/bind.rs`). Written the
/// other way round the snippet does not compile — `Read: CrudService` and
/// `UsersService: ActionMarker` both fail — and the prose form `Bind<S, A>`
/// teaches the wrong rule to every page that repeats it. 1.1.1 shipped it
/// reversed across ~10 pages, so the shape is gated rather than trusted.
/// Same defect on the proof the binder returns: `Authorized<A, E>`
/// (`nest-rs-seaorm/src/service.rs`), action first, entity second.
const BIND_ORDER =
  /\b(?:[Bb]ind(?:_required)?(?:::)?<\s*(?:S|[A-Z]\w*Service)|Authorized<\s*(?:E|[A-Z]\w*Entity))\b/g;

/// A queue is named by its `QueueName` **type**, never a string — the macro
/// refuses both string spellings by name, so the regex catches both:
/// `#[process(queue = "audio")]` shipped in 1.1.1 across ~10 places on pages
/// that predated `QueueName`, and the *positional* `#[process("posts.publish")]`
/// survived that fix on a page outside the queue section. A reader following
/// either wrote a consumer that would not compile. Gated rather than trusted.
const QUEUE_STRING_FORM = /#\[process\(\s*(?:queue\s*=\s*)?"/g;

/// The producer half of the same rule: `push(name, job)` and `of::<…>(…)` are
/// the runtime-name escape hatch, not the default — a page teaching them as
/// *the* way to enqueue opts the reader out of the very check the type exists
/// to provide, and does it silently.
const QUEUE_UNTYPED_PUSH = /\.(?:of::<[^>]*>\(|push\(\s*[A-Z_]{3,}\b)/g;

/// A test target is a **directory** — `tests/<suite>/main.rs`. Cargo compiles a
/// flat `tests/<x>.rs` as its own binary, so a suite scattered across sibling
/// files escapes the `binary(e2e)` gate and relinks once per file. The section
/// index taught the flat form three times while `testing/integration.mdx` next
/// door explained why it does not exist, and neither workspace holds one.
///
/// Scoped to where a page **prescribes** a location — a fence `title=` and a
/// table cell — because naming the flat form is exactly how `e2e.mdx` refuses
/// it, and a check that cannot tell the two apart makes the refusal unwritable.
const FLAT_TEST_TARGET = /\btests\/[A-Za-z0-9_*]+\.rs\b/g;

/// Two facts a fence titled with a real `demo/` file cannot contradict. Titles
/// cite the *user's* workspace shape, so the prefix is mapped back onto `demo/`;
/// a title whose file does not exist there is a fictional snippet, which the
/// rule does not reach.
///
/// The full STYLE.md §C rule is byte-for-byte, and it is deliberately **not**
/// what runs here: most fences are honest excerpts written before the marker
/// convention, so the strict form reports 134 pages at once and the signal is
/// gone. These two are exact, and each was a shipped defect:
///
/// - **A comment.** `demo/` carries none — owner's rule, no exceptions — so a
///   page quoting it with a `///` publishes code the repo forbids writing,
///   under a title asserting the repo contains it. `producing-jobs.mdx` added
///   two.
/// - **A port.** `issuer-and-resource-server.mdx` pinned `3000` in a block
///   titled `apps/api/src/module.rs` while the app listens on `3002` — and
///   `curl`ed `3002` forty lines down.
const DEMO_TITLE_PREFIXES = [
  [/^crates\//, 'demo/crates/'],
  [/^apps\//, 'demo/apps/'],
];

function demoPathFor(title) {
  const clean = title.replace(/\s*\(abridged\)\s*$/, '').trim();
  for (const [re, prefix] of DEMO_TITLE_PREFIXES) {
    if (re.test(clean)) return join(REPO_ROOT, clean.replace(re, prefix));
  }
  return null;
}

/// The `port:` a demo file pins, or `null` when it pins none — `undefined` when
/// the path is not a file. Cached on the *derived* value rather than the source:
/// 64 distinct files back 143 titled fences corpus-wide, and one app module is
/// quoted 18 times.
const DEMO_PORTS = new Map();
function demoPort(abs) {
  if (!DEMO_PORTS.has(abs)) {
    DEMO_PORTS.set(
      abs,
      existsSync(abs) ? (readFileSync(abs, 'utf8').match(/\bport:\s*(\d+)/)?.[1] ?? null) : undefined,
    );
  }
  return DEMO_PORTS.get(abs);
}

function fenceTitleDrift(blocks) {
  const out = [];
  for (const block of blocks) {
    const title = block.info.match(/title="([^"]+)"/)?.[1];
    if (!title) continue;
    const abs = demoPathFor(title);
    if (!abs) continue;
    const real = demoPort(abs);
    if (real === undefined) continue;

    const comment = block.body.match(/^\s*(\/\/\/?!?[^\n]*)/m);
    // `// …` is the elision mark an excerpt uses; it claims nothing about the file.
    if (comment && !/^\s*\/\/\s*[….]/.test(comment[1])) {
      out.push(`${title} is quoted with a comment (\`${comment[1].trim()}\`) — the demo `
        + 'workspace carries none, so the file does not contain that line');
    }

    const shown = block.body.match(/\bport:\s*(\d+)/)?.[1];
    if (shown && real !== null && shown !== real) {
      out.push(`${title} pins port ${shown}, the app listens on ${real}`);
    }
  }
  return out;
}

/// The lines that prescribe a path: a fence title, and a table row.
function prescriptiveLines(src) {
  return src.split('\n').filter((line) => /^\s*\|/.test(line) || /```[^\n]*title=/.test(line));
}

/// The binding the crate's own panic text tells a reader to write when
/// `OpenTelemetryModule` was imported without `OpenTelemetry::init`. Read out of
/// the panic rather than restated: 1.3.0 corrected that message to `_otel` and
/// left the page's canonical `main` on the old `_opentelemetry`, so the reader
/// who tripped the panic was sent to a line the example he started from did not
/// contain.
function otelGuardBinding() {
  const m = frameworkSource('crates/nest-rs-opentelemetry/src/module.rs')
    .match(/Add `let (\w+) =/);
  // Fail closed rather than skip: a reworded panic means the rule no longer has
  // a name to check against, and silently dropping the check is how the two
  // halves drifted apart in the first place.
  if (!m) {
    throw new Error('nest-rs-opentelemetry\'s boot panic no longer reads "Add `let <binding> ='
      + '" — teach `otelGuardBinding` the new wording, do not delete the check');
  }
  return m[1];
}

const OTEL_BINDING = otelGuardBinding();

/// The `#[config]` structs whose page publishes a key table, keyed by page and
/// carrying the source that owns the fields. A table read as exhaustive — every
/// one of these pages says so above it — that omits a field publishes a key the
/// reader has no way to learn about. `/storage/` shipped 2.0.0 listing five of
/// `StorageConfig`'s seven fields, and the missing `ALLOW_HTTP` is the one that
/// decides a boot refusal.
///
/// Kept a list rather than derived: mentioning `NESTRS_X__…` is not publishing a
/// key table, and no grep separates the two. Add a page when it grows one.
const CONFIG_TABLES = new Map([
  ['storage/index.mdx', { source: 'crates/nest-rs-storage/src/config.rs', struct: 'StorageConfig' }],
]);

/// The field names of a `#[config]` struct, plus whether its `defaults()` is
/// profile-dependent — the second thing `/storage/` got wrong, publishing the
/// dev branch of a profile-split default as *the* default.
function configFields({ source, struct }) {
  const src = frameworkSource(source);
  const body = src.match(new RegExp(`struct ${struct} \\{([\\s\\S]*?)\\n\\}`));
  // Fail closed: a moved struct means the rule has nothing to check against.
  if (!body) {
    throw new Error(`no \`struct ${struct}\` in ${source} — teach \`CONFIG_TABLES\` `
      + 'where it moved, do not delete the check');
  }
  const fields = [...body[1].matchAll(/^\s*pub\s+(\w+)\s*:/gm)].map((m) => m[1]);
  const defaults = src.match(/fn defaults\(\)[\s\S]*?\n    \}/);
  return { fields, profileSplit: !!defaults && /dev_profile\(\)/.test(defaults[0]) };
}

/// A snippet that keeps the OTel guard alive — the binding has to be the one the
/// panic names, or the two halves of the page contradict each other.
const OTEL_INIT = /\blet\s+(\w+)\s*=\s*(?:nest_rs_opentelemetry::)?OpenTelemetry::init\s*\(/g;

/// A module page publishes its install stanza **twice** — a `cargo add` line in
/// a `bash` block and a `[dependencies]` block in `toml` — and the reader runs
/// the first one. Three pages shipped 1.3.0 with the two disagreeing:
/// `/configuration/` said `cargo add validator` (which resolves 0.21) beside a
/// `validator = "0.20"` pin, `/database/` dropped every feature from its
/// `cargo add`, and `/mcp/` listed neither crate `#[mcp]` expands to. In each
/// case the page's own opening snippet failed to compile after its own install
/// step. Two sources of truth for one fact drift, so they are held equal here:
/// same crates, same features, same `default-features`, and an explicit
/// `@<req>` whenever the manifest constrains beyond the major — a bare
/// `cargo add` takes the newest major, which is the validator trap exactly.
function cargoAddInvocations(blocks) {
  const out = [];
  for (const block of blocks) {
    if (!SHELL_INFO.test(block.info)) continue;
    for (const line of shellLines(block.body)) {
      if (!/^cargo\s+add\b/.test(line)) continue;
      const tokens = line.split(/\s+/).slice(2);
      const pkgs = [];
      const features = [];
      let noDefault = false;
      for (let i = 0; i < tokens.length; i++) {
        const t = tokens[i];
        if (t === '--no-default-features') { noDefault = true; continue; }
        if (t === '--features' || t === '-F') { features.push(...splitFeatures(tokens[++i])); continue; }
        if (t.startsWith('--features=')) { features.push(...splitFeatures(t.slice(11))); continue; }
        if (t.startsWith('-')) continue; // --dev, --build, --optional, …
        const at = t.lastIndexOf('@');
        pkgs.push(at > 0
          ? { name: t.slice(0, at), req: t.slice(at + 1) }
          : { name: t, req: null });
      }
      out.push({ line, pkgs, features, noDefault });
    }
  }
  return out;
}

/// A comma-separated feature list in either form the two artifacts write it —
/// `--features a,b` on the command line, `features = ["a", "b"]` in the
/// manifest — normalized so the two can be compared.
function splitFeatures(raw) {
  return (raw ?? '').split(',')
    .map((f) => f.trim().replace(/^["']|["']$/g, ''))
    .filter(Boolean);
}

/// The `[dependencies]` table of one `toml` block, or null when the block is not
/// an install stanza — no `[dependencies]` header, or entries written
/// `workspace = true` (a workspace member's manifest, which carries no version
/// and no `cargo add` counterpart).
function parseDependencies(body) {
  const start = body.indexOf('[dependencies]');
  if (start === -1) return null;
  const section = body.slice(start + '[dependencies]'.length)
    .split(/\n\[/)[0]                 // up to the next table header
    .replace(/#.*$/gm, '');           // comments, including a trailing one
  const deps = new Map();
  // Split on the start of the next `name =`, so an entry spanning several lines
  // (sea-orm's feature list does) stays one chunk without tracking brackets.
  for (const chunk of section.split(/\n(?=[A-Za-z0-9_-]+\s*=)/)) {
    const entry = chunk.trim().match(/^([A-Za-z0-9_-]+)\s*=\s*([\s\S]+)$/);
    if (!entry) continue;
    const [, name, value] = entry;
    if (/\bworkspace\s*=\s*true/.test(value)) return null;
    const req = value.startsWith('{')
      ? (value.match(/version\s*=\s*"([^"]+)"/) || [])[1] ?? null
      : (value.match(/^"([^"]+)"/) || [])[1] ?? null;
    deps.set(name, {
      req,
      features: splitFeatures((value.match(/features\s*=\s*\[([\s\S]*?)\]/) || [])[1]),
      noDefault: /default-features\s*=\s*false/.test(value),
    });
  }
  return deps.size ? deps : null;
}

/// Whether `cargo add <name>` has to carry an explicit `@<req>`. A bare add
/// resolves the newest major, so anything the manifest constrains past the
/// major (`0.20`, `2.0`, `0.1`) has to say so. `nest-rs*` pins are out of scope
/// — `version-pin` already ties them to the release the repo builds, and the
/// newest published major is that release by construction.
function needsPinnedAdd(name, req) {
  if (!req || name.startsWith('nest-rs')) return false;
  return bareReq(req).includes('.');
}

function sortedFeatures(list) {
  return [...list].sort().join(', ');
}

/// The `install-stanza` details for one page, or none when the page publishes
/// its install list only once (a `## Install` with no manifest block, or a
/// manifest with no `cargo add`) — there is nothing to hold equal.
function installStanzaViolations(blocks) {
  const installBlocks = blocks.filter((b) => b.section === 'Install');
  const invocations = cargoAddInvocations(installBlocks);
  const manifest = new Map();
  for (const block of installBlocks) {
    if (!/^toml\b/.test(block.info)) continue;
    const deps = parseDependencies(block.body);
    if (deps) for (const [name, dep] of deps) manifest.set(name, dep);
  }
  if (!invocations.length || !manifest.size) return [];

  const out = [];
  const installed = new Map();
  for (const inv of invocations) {
    if ((inv.features.length || inv.noDefault) && inv.pkgs.length > 1) {
      out.push(`\`${inv.line}\` applies its features to every package it names — `
        + 'split it, one crate per `cargo add`');
    }
    for (const p of inv.pkgs) {
      installed.set(p.name, { req: p.req, features: inv.features, noDefault: inv.noDefault });
    }
  }
  for (const [name, dep] of manifest) {
    const got = installed.get(name);
    if (!got) {
      out.push(`${name} is in the Cargo.toml block, no \`cargo add\` installs it`);
      continue;
    }
    const asked = sortedFeatures(got.features);
    const declared = sortedFeatures(dep.features);
    if (asked !== declared) {
      out.push(`${name}: \`cargo add\` asks for [${asked}], the manifest declares [${declared}]`);
    }
    if (got.noDefault !== dep.noDefault) {
      out.push(dep.noDefault
        ? `${name}: the manifest sets \`default-features = false\` — \`cargo add\` needs \`--no-default-features\``
        : `${name}: \`cargo add --no-default-features\` has no counterpart in the manifest`);
    }
    if (got.req && dep.req && got.req !== dep.req) {
      out.push(`${name}: \`cargo add ${name}@${got.req}\` against a manifest pin of ${dep.req}`);
    } else if (!got.req && needsPinnedAdd(name, dep.req)) {
      out.push(`${name}: the manifest pins ${dep.req}, so the line has to say `
        + `\`${name}@${dep.req}\` — a bare \`cargo add\` takes the newest major`);
    }
  }
  for (const name of installed.keys()) {
    if (!manifest.has(name)) {
      out.push(`${name} is installed by \`cargo add\`, absent from the Cargo.toml block`);
    }
  }
  return out;
}

/// The two rules read out of the framework's own sources, in **one** pass over
/// `crates/` — the tree is ~560 files, and walking it twice to read it twice
/// cost more than linting the whole docs corpus.
///
/// `layerSubtraits` — every trait declared `: Layer`. The blanket impl a reader
/// expects does not exist (the marker carries the per-layer scope metadata, so
/// it is opted into per type), and a page that shows the sub-trait impl and
/// drops `impl Layer for T {}` hands out an `E0277` naming `nest_rs_core::Layer`,
/// which does not say "add a one-line impl". `/fundamentals/middleware/` shipped
/// 2.0.0 that way while the guard snippet on the same page carried its line, and
/// `/fundamentals/interceptors/` quoted a real framework file with the line
/// stripped. Derived, because a hand-written list is wrong the day a sub-trait
/// is added: the first version listed four and missed `GlobalPipe`.
///
/// `decorators` — every decorator the umbrella exports, from the `*-macros`
/// crate roots. Also derived, and it has to be exact in both directions: a name
/// missing here hides a broken snippet, a name that is not a macro flags a
/// working one. Only `#[proc_macro_attribute]` entries count — the attributes an
/// orchestrator consumes (`#[query]`, `#[get]`, `#[on_module_init]`,
/// `#[public]`) are inert tokens read by the host macro, so they resolve without
/// an import of their own and must never be demanded.
/// Every `pub trait` a source declares, with the method names its body holds.
///
/// One generator, two callers — the canon side ([`frameworkRules`]) and the page
/// side (check 20). That is the point rather than a line saving: check 20 *is* a
/// diff between the two, so teaching one side about a `where` clause or an
/// associated const and not the other would make it report invented methods that
/// are not invented, or go silent. Nothing would catch the skew.
function* traitDecls(src) {
  for (const m of src.matchAll(/pub trait (\w+)(?:<[^>]*>)?\s*(?::[^{]*)?\{/g)) {
    const start = m.index + m[0].length;
    let depth = 1;
    let i = start;
    while (i < src.length && depth > 0) {
      if (src[i] === '{') depth += 1;
      else if (src[i] === '}') depth -= 1;
      i += 1;
    }
    // A doc comment names methods too, and a default body may nest an `fn`.
    const decl = src
      .slice(start, i - 1)
      .replace(/^\s*\/\/.*$/gm, '')
      .replace(/\{[^{}]*\}/g, '{}');
    yield { name: m[1], methods: [...decl.matchAll(/\bfn\s+(\w+)/g)].map((f) => f[1]) };
  }
}

/// `traitMethods` — every `pub trait` the framework ships, mapped to the method
/// names its body declares, so a page publishing a signature can be diffed
/// against the real one. `/fundamentals/exception-filters/` published `Filter`
/// and `ExceptionFilter` with **three** methods each — four names that exist
/// nowhere under `crates/` — then spent an Aside explaining why the four do not
/// work. A reader who wrote one got `E0407`, and the page was the only source
/// that had ever claimed the method. One direction only: a page may abridge a
/// trait, it may never invent a method.
function frameworkRules() {
  const traits = new Set();
  const decorators = new Set();
  const traitMethods = new Map();
  // The landing's `N+ tests` floor, counted in the one walk that already reads
  // every framework file — unit tests in `src/`, suites under `tests/`, alike.
  let tests = 0;
  for (const file of walk(join(REPO_ROOT, 'crates'), ['.rs'])) {
    const src = readFileSync(file, 'utf8');
    tests += (src.match(/#\[(?:tokio::)?test\]|#\[rstest\]/g) || []).length;
    for (const m of src.matchAll(/pub trait (\w+)\s*:\s*Layer\b/g)) traits.add(m[1]);
    // A trait split across crates (a feature-gated half) contributes its own
    // names rather than replacing the set.
    for (const { name, methods } of traitDecls(src)) {
      const known = traitMethods.get(name) ?? new Set();
      for (const method of methods) known.add(method);
      traitMethods.set(name, known);
    }
    if (!/-macros[\\/]src[\\/]lib\.rs$/.test(file)) continue;
    for (const m of src.matchAll(/#\[proc_macro_attribute\]\s*pub fn (\w+)/g)) {
      decorators.add(m[1]);
    }
  }
  // Fail closed rather than skip: an empty set silently disables the check.
  if (!traits.size) {
    throw new Error('no `pub trait <T>: Layer` found under crates/ — teach `frameworkRules` '
      + 'where the Layer System moved, do not delete the check');
  }
  if (!decorators.size) {
    throw new Error('no `#[proc_macro_attribute]` found under crates/*-macros — teach '
      + '`frameworkRules` where the decorators moved, do not delete the check');
  }
  if (!traitMethods.size) {
    throw new Error('no `pub trait` found under crates/ — teach `traitDecls` where the '
      + 'framework moved, do not delete the check');
  }
  if (!tests) {
    throw new Error('no test function found under crates/ — teach `frameworkRules` how the '
      + 'suites are spelled now, do not delete the check');
  }
  return { traits, decorators: [...decorators], traitMethods, tests };
}

const {
  traits: LAYER_SUBTRAITS,
  decorators: DECORATORS,
  traitMethods: FRAMEWORK_TRAITS,
  tests: TEST_COUNT,
} = frameworkRules();

/// A rust snippet — the fence language the code-truth checks read.
const RUST_INFO = /^rust\b/;

/// One pair of patterns per decorator — the applied attribute and the `use` that
/// would import it. Built once: they depend only on the decorator name, and
/// `missingDecoratorImports` would otherwise recompile both for every decorator
/// on every rust block on every page.
const DECORATOR_PATTERNS = DECORATORS.map((d) => ({
  name: d,
  applied: new RegExp(`^\\s*#\\[${d}[\\](]`, 'm'),
  imported: new RegExp(`use [^;]*\\b${d}\\b[^;]*;`, 's'),
}));

/// A snippet showing **no** `use` at all is read as a fragment; one that shows
/// its imports is read as complete, and a reader pastes it whole. Twenty-four
/// blocks imported the types they name and dropped the decorator that shapes
/// them — `use nest_rs::openapi::OpenApiModule;` above a `#[module(...)]` with
/// no `use nest_rs::core::module;`, which is `error: cannot find attribute
/// `module` in this scope` on the first build. `configuration/` and
/// `http/configuration.mdx` held four and three of them: the pages opened
/// precisely to copy a stanza out of.
///
/// A `prelude::*` covers every decorator at once, so a block that has one is
/// complete by construction.
function missingDecoratorImports(blocks) {
  const out = [];
  for (const block of blocks) {
    if (!RUST_INFO.test(block.info)) continue;
    if (!/^\s*use\s+/m.test(block.body)) continue;
    if (/prelude::\*/.test(block.body)) continue;
    for (const { name, applied, imported } of DECORATOR_PATTERNS) {
      if (!applied.test(block.body) || imported.test(block.body)) continue;
      out.push(`#[${name}] is used but never imported — the block shows its other `
        + `imports, so it reads as pasteable and is not`);
    }
  }
  return out;
}

/// What the page's rust blocks *declare*: the types it defines (the only ones it
/// owes an `impl Layer` for — a snippet illustrating the framework's own
/// `AuthnGuard` names it without declaring it, and that impl lives in the
/// framework) and every `impl <Trait> for <Type>` it writes.
function rustDeclarations(blocks) {
  const types = new Set();
  const impls = [];
  for (const block of blocks) {
    if (!RUST_INFO.test(block.info)) continue;
    for (const m of block.body.matchAll(
      /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum)\s+(\w+)/gm)) types.add(m[1]);
    for (const m of block.body.matchAll(
      /^\s*impl(?:<[^>]*>)?\s+([A-Za-z_]\w*)\s+for\s+([A-Za-z_]\w*)/gm)) {
      impls.push({ trait: m[1], type: m[2] });
    }
  }
  return {
    types,
    impls,
    implementorsOf: (t) => new Set(impls.filter((i) => i.trait === t).map((i) => i.type)),
  };
}

/// An `ExceptionFilter` claims its exception by **downcast**, off an error that
/// is already a `poem::Error` — so the exception type needs `ResponseError` to
/// reach the chain at all. `/fundamentals/exception-filters/` shipped 2.0.0
/// defining `DomainError` with the filter but never the impl, and never the
/// handler that raises it; a reader following it got `E0277` on `IntoResult`,
/// which names neither `ResponseError` nor the status it supplies. The demo's
/// `PostError`, cited two sections below on the same page, has the impl.
const EXCEPTION_ASSOC = /^\s*type\s+Exception\s*=\s*([A-Za-z_]\w*)\s*;/gm;

/// `CLAUDE.md`: *metadata is mandatory — a bare log is a defect*, because those
/// are the events queried under incident. The scaffolds hold this at zero
/// (`nest-rs-cli/src/templates/mod.rs` asserts it over every template); the
/// pages a reader copies from have to as well.
///
/// A match *is* the violation: the pattern runs from the macro call, past an
/// optional `target:`, straight into the message literal, so anything between —
/// `k = v`, the `%v`/`?v` sigils, the bare shorthand — makes it fail. `\s*`
/// spans newlines deliberately: the corpus's multi-field logs are the ones
/// rustfmt broke across lines, and they are where a dropped field would hide.
/// Anchoring on the macro-call shape keeps prose mentioning `tracing::` out.
const BARE_LOG = /tracing::\w+!\(\s*(?:target:\s*"([^"]*)"\s*,\s*)?"/g;

/// Marks a snippet as a handler — the only layer where the check above applies.
/// A **service** method returning `ServiceError` converts `DbErr` through `?`
/// legitimately, and that is where the conversion belongs: the exemplar's
/// services return the wire type, so a handler is a one-line delegation.
const HANDLER_SNIPPET = /#\[(?:get|post|put|patch|delete|sse)\(|#\[(?:query|mutation)\]/;

/// Every fenced block, tagged with the `##` section it sits under. Most checks
/// ignore the section; `install-stanza` is scoped by it, because "the install
/// list" means the one under `## Install` and not a variant manifest shown
/// further down the page.
function fencedBlocks(src) {
  const headings = [...src.matchAll(/^##\s+(.*)$/gm)];
  const out = [];
  const re = /```([^\n]*)\n([\s\S]*?)```/g;
  let m;
  let h = 0;
  while ((m = re.exec(src)) !== null) {
    while (h < headings.length && headings[h].index < m.index) h += 1;
    out.push({
      info: m[1].trim(),
      body: m[2],
      section: h > 0 ? headings[h - 1][1].trim() : null,
    });
  }
  return out;
}

/// The fence languages that hold a pasteable shell command.
const SHELL_INFO = /^(bash|sh|shell|console|zsh)\b/;

/// The lines of a shell block as a reader would run them: continuations folded
/// so an argument on the next line still belongs to its command, and a `$`
/// prompt stripped.
function shellLines(body) {
  return body.replace(/\\\n\s*/g, ' ').split('\n')
    .map((line) => line.replace(/^\s*\$\s*/, '').trim());
}

/// A version requirement without its comparison operator.
function bareReq(req) {
  return req.replace(/^[\^~=]/, '');
}

/// The guarded route root a `curl` targets, or null — the command names no
/// concrete host (`…/posts/$ID` is elided shorthand, not something a reader
/// pastes) or hits a root outside the canon. A `v\d+` prefix is skipped:
/// `/v1/posts` is the `posts` controller under a version prefix.
function guardedCurlRoot(command) {
  const m = command.match(
    /(?:https?:\/\/)?(?:localhost|127\.0\.0\.1|\[::1\]):\d+\/(?:v\d+\/)?([^/?#\s'"|)]+)/);
  return m && GUARDED_ROUTE_ROOTS.has(m[1]) ? m[1] : null;
}

/// Every file under `dir` with one of `exts` — the docs corpus by default, and
/// the framework's own sources for the checks that derive their rule from it.
function walk(dir, exts = ['.md', '.mdx']) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p, exts));
    else if (exts.some((ext) => name.endsWith(ext))) out.push(p);
  }
  return out;
}

// Remove fenced code blocks and inline code so prose checks don't fire inside code.
function stripCode(src) {
  return src
    .replace(/```[\s\S]*?```/g, '')
    .replace(/`[^`\n]*`/g, '');
}

/// A page's id — its path under `CONTENT`, slash-separated on every platform.
function relOf(absPath) {
  return relative(CONTENT, absPath).split('\\').join('/');
}

/// Every page's frontmatter `title`, filled as `lintFile` walks. Accumulated
/// rather than re-read: `lintFile` already has the source and already parsed the
/// frontmatter, and a second pass would be a second place deciding what counts
/// as a page.
const PAGE_TITLES = new Map();

function frontmatter(src) {
  const m = src.match(/^---\n([\s\S]*?)\n---/);
  return m ? m[1] : null;
}

function lintFile(absPath) {
  const rel = relOf(absPath);
  const src = readFileSync(absPath, 'utf8');
  const prose = stripCode(src);
  const blocks = fencedBlocks(src);
  const v = [];
  const add = (rule, detail) => v.push(`${rel}::${rule}::${detail}`);

  // 1. Frontmatter description.
  const fm = frontmatter(src);
  if (fm === null) {
    add('frontmatter', 'missing frontmatter');
  } else {
    const title = fm.match(/^title:\s*(.+?)\s*$/m)?.[1].replace(/^["']|["']$/g, '');
    if (title) {
      if (!PAGE_TITLES.has(title)) PAGE_TITLES.set(title, []);
      PAGE_TITLES.get(title).push(rel);
    }
    const dm = fm.match(/^description:\s*(.*)$/m);
    if (!dm) {
      add('description', 'missing');
    } else {
      let raw = dm[1].trim();
      const quoted = /^".*"$/.test(raw) || /^'.*'$/.test(raw);
      const value = quoted ? raw.slice(1, -1) : raw;
      if (!quoted && /\s#/.test(raw)) add('description', 'unquoted-hash (YAML truncation)');
      if (value.length > 160) add('description', `too-long (${value.length}>160)`);
    }
    // 1b. A tier places a page inside its section's groups. A page that is in no
    // section — the hand-listed roots of `Start here` and `Reference` — has no
    // group to be placed in, so the key would sit there saying nothing.
    if (!rel.includes('/') && /^tier:/m.test(fm)) {
      add('tier', 'a page in no section declares a tier — nothing would group it');
    }
  }

  // 2. Banned heading variants (## or ###).
  for (const line of src.split('\n')) {
    const h = line.match(/^#{2,3}\s+(.*)$/);
    if (h && BANNED_HEADINGS.includes(h[1].trim().toLowerCase())) {
      add('heading', h[1].trim());
    }
  }

  // 3. Banned prose words.
  for (const w of BANNED_WORDS) {
    const re = new RegExp(`\\b${w}\\b`, 'i');
    if (re.test(prose)) add('banned-word', w);
  }
  // Exclamation marks in prose (exclude "!=" and markup).
  if (/[A-Za-z0-9,)"'’]!(\s|$)/m.test(prose)) add('exclamation', 'prose ! found');

  // 4. Closing "## Going further".
  if (!GOING_FURTHER_EXEMPT.has(rel)) {
    if (!/^##\s+Going further\s*$/m.test(src)) add('going-further', 'missing closing block');
  }

  // 4b. A page that restates a shipped file may not drift from it.
  const mirror = MIRRORED_PAGES.get(rel);
  if (mirror) {
    MIRRORS_SEEN.add(rel);
    for (const detail of mirror.check(src)) add(mirror.rule, detail);
  }

  // 5. ≤3 Asides.
  const asides = (src.match(/<Aside\b/g) || []).length;
  if (asides > 3) add('asides', `${asides} > 3`);

  // 6. Example-canon ban list.
  for (const term of CANON_BANLIST) {
    if (new RegExp(`\\b${term}\\b`).test(src)) add('canon', term);
  }
  for (const [re, label] of CANON_SHAPES) {
    const hit = src.match(re);
    if (hit) add('canon', `${label}: ${hit[0]}`);
  }

  // 7. `nest-rs*` pins track the version the repo builds.
  for (const m of src.matchAll(NEST_RS_PIN)) {
    const pinned = bareReq(m[1]);
    const [major, minor] = pinned.split('.');
    if (`${major}.${minor}` !== VERSION_REQ) {
      add('version-pin', `${m[0].split('=')[0].trim()} pins ${pinned}, workspace is ${VERSION_REQ}`);
    }
  }

  // 8. The by-id binder's type parameters, in the order the code declares.
  for (const m of src.matchAll(BIND_ORDER)) {
    add('bind-order', `${m[0]}… — the action marker comes first`);
  }

  // 9. A queue is named by its `QueueName` type on both sides.
  for (const m of src.matchAll(QUEUE_STRING_FORM)) {
    add('queue-name', `${m[0]}…" — name the queue by its QueueName type`);
  }
  for (const m of src.matchAll(QUEUE_UNTYPED_PUSH)) {
    add('queue-name', `${m[0]}… — enqueue with push_to::<Q>, not an untyped name`);
  }

  for (const block of blocks) {
    const shell = SHELL_INFO.test(block.info);

    // 10. A pasteable `curl` against a guarded route carries a bearer — unless
    // the block is documenting the denial itself.
    if (shell && !/\b(401|403|Unauthorized|Forbidden)\b/.test(block.body)) {
      for (const line of shellLines(block.body)) {
        if (!/\bcurl\b/.test(line) || /authorization:/i.test(line)) continue;
        const root = guardedCurlRoot(line);
        if (root) add('unauthed-curl', `/${root} without a bearer`);
      }
    }

    // 11. A handler snippet that `?`s a `CrudService` read does not compile.
    if (RUST_INFO.test(block.info) && HANDLER_SNIPPET.test(block.body)) {
      for (const line of block.body.split('\n')) {
        if (UNMAPPED_CRUD_READ.test(line) && !line.includes('map_err')) {
          add('crud-error', `unmapped DbErr: ${line.trim()}`);
        }
      }
    }
  }

  // 12. The OTel guard binds the name the crate's boot panic prescribes.
  for (const m of src.matchAll(OTEL_INIT)) {
    if (m[1] !== OTEL_BINDING) {
      // No `::` in the detail — the console splits a violation on it.
      add('otel-guard', `\`let ${m[1]} =\` binds the OTel guard, but the boot panic tells the `
        + `reader to write \`let ${OTEL_BINDING} =\``);
    }
  }

  // 13. Under `## Install`, the `cargo add` line and the `[dependencies]` block
  // say the same thing.
  for (const detail of installStanzaViolations(blocks)) add('install-stanza', detail);

  // 14. A snippet that shows its imports imports the decorator it illustrates.
  for (const detail of missingDecoratorImports(blocks)) add('decorator-import', detail);

  // 15. A page-defined type implementing a Layer sub-trait carries `impl Layer`.
  const rust = rustDeclarations(blocks);
  const hasLayer = rust.implementorsOf('Layer');
  for (const { trait: t, type } of rust.impls) {
    if (!LAYER_SUBTRAITS.has(t) || !rust.types.has(type) || hasLayer.has(type)) continue;
    add('layer-impl', `impl ${t} for ${type} without \`impl Layer for ${type} {}\` — `
      + `${t} is declared \`: Layer\` and there is no blanket impl`);
  }

  // 16. An `ExceptionFilter`'s exception reaches the chain as a `poem::Error`.
  const hasResponseError = rust.implementorsOf('ResponseError');
  for (const m of src.matchAll(EXCEPTION_ASSOC)) {
    const exception = m[1];
    if (!rust.types.has(exception) || hasResponseError.has(exception)) continue;
    add('exception-response-error', `${exception} is claimed by an ExceptionFilter but `
      + `implements no ResponseError — the filter catches by downcast off an error that is `
      + `already a poem-Error, so the handler raising it does not compile`);
  }

  // 17. Every documented log carries at least one structured field — a match on
  // `BARE_LOG` is the violation itself.
  for (const m of src.matchAll(BARE_LOG)) {
    // No `::` in the detail — the console splits a violation on it.
    const where = m[1] ? `on target ${m[1].replace(/::/g, '.')}` : 'with no target';
    add('bare-log', `the log ${where} carries no structured field`);
  }

  // 18. A config-key table is exhaustive, and publishes both branches of a
  // profile-dependent default.
  const configTable = CONFIG_TABLES.get(rel);
  if (configTable) {
    const { fields, profileSplit } = configFields(configTable);
    for (const field of fields) {
      const key = field.toUpperCase();
      if (!src.includes(`\`${key}\``)) {
        add('config-table', `${configTable.struct}.${field} has no \`${key}\` row — the `
          + 'table is published as the full key list');
      }
    }
    if (profileSplit && !src.includes('staging/production')) {
      // No `::` in the detail — the console splits a violation on it.
      add('config-table', `${configTable.struct}'s defaults() branches on the profile, but `
        + 'the page never names staging/production — it publishes the dev branch as the default');
    }
  }

  // 19. One spelling for a pinned config: `for_root(x)`, never `for_root(Some(x))`.
  for (const m of src.matchAll(/\bfor_root\(\s*Some\(/g)) {
    add('for-root-form', `${m[0]}…)) — the seam takes \`impl Into<Option<C>>\`, so the demo `
      + 'and every scaffold write the bare value; keep `None` for the env-only call');
  }

  // 20. A fence titled with a real file quotes it.
  for (const detail of fenceTitleDrift(blocks)) add('fence-title', detail);

  // 21. A published trait signature does not invent a method.
  for (const block of blocks) {
    if (!RUST_INFO.test(block.info)) continue;
    for (const { name: trait, methods } of traitDecls(block.body)) {
      const real = FRAMEWORK_TRAITS.get(trait);
      // Engage only where the two share a method. 77 bare trait names collide
      // across `crates/` — `Config`, `Filter`, `Module`, `Job`, `Registry` — so a
      // page defining its own `pub trait Registry` for an example is not making
      // a claim about `nest-rs-ws`'s, and diffing it against one is pure noise.
      if (!real || !methods.some((m) => real.has(m))) continue;
      for (const method of methods.filter((m) => !real.has(m))) {
        add('trait-surface', `${trait}-${method} is published on the page but the trait under `
          + 'crates/ declares no such method — writing it is an E0407');
      }
    }
  }

  // 22. A test target is a directory, never a flat `tests/<x>.rs`.
  for (const line of prescriptiveLines(src)) {
    for (const m of line.matchAll(FLAT_TEST_TARGET)) {
      add('test-layout', `${m[0]} is a flat test target — a suite is a directory, `
        + 'tests/<suite>/main.rs');
    }
  }

  return v;
}

/// 19. The Basics / All options split (STYLE.md §G), checked per **section** —
/// the one invariant no single page can carry. A section past the threshold
/// presents two lists, so every page in it says which one it is in, and both
/// have to hold something: a section that declares one tier is the flat list
/// with a header on it, which is the state the split exists to replace.
///
/// Below the threshold the rule inverts — a `tier` there is a page claiming a
/// grouping the sidebar will not render, and the reader never sees it.
/// Violations are filed against the page that carries the wrong frontmatter,
/// and a missing tier against the `index` that frames the section.
function lintSections() {
  const out = [];
  const add = (rel, detail) => out.push(`${rel}::tier::${detail}`);

  for (const { dir, index, pages, tiered } of sections()) {
    if (!tiered) {
      const why = UNTIERED_SECTIONS.has(dir)
        ? `${dir}/ is an ordered path, exempt from the split at any size`
        : `${dir}/ has ${pages.length} pages, under the ${TIER_THRESHOLD} a split needs`;
      for (const page of pages.filter((p) => p.tier)) {
        add(page.rel, `\`tier: ${page.tier}\` on a page in a flat section — ${why}`);
      }
      continue;
    }
    for (const page of pages) {
      if (!page.tier) {
        add(page.rel, `no tier — ${dir}/ presents ${pages.length} pages in two groups, so `
          + `each one declares \`tier: ${TIERS.join('` or `tier: ')}\``);
      } else if (!TIERS.includes(page.tier)) {
        add(page.rel, `unknown tier \`${page.tier}\` — the tiers are `
          + TIERS.map((t) => `\`${t}\``).join(' and '));
      }
    }
    for (const tier of TIERS.filter((t) => !pages.some((p) => p.tier === t))) {
      add(index ? index.rel : `${dir}/index.mdx`,
        `${dir}/ declares no ${TIER_LABELS[tier]} page — a section split into one tier is `
        + 'the flat list with a header on it');
    }
  }
  return out;
}

/// No two pages carry the same `title`. In search (Pagefind) the title is all a
/// reader sees, so two "Health" pages — one of them the stale one — is the exact
/// configuration where the wrong page wins. The fix is a qualified `title` plus
/// a short `sidebar.label`, which leaves the navigation unchanged.
function lintTitles() {
  return [...PAGE_TITLES]
    .filter(([, pages]) => pages.length > 1)
    .flatMap(([title, pages]) => pages.map((rel) => `${rel}::title::\`${title}\` is also the `
      + `title of ${pages.filter((p) => p !== rel).join(', ')} — qualify it and keep the short `
      + 'name as `sidebar.label`'));
}

const current = [...PAGES.flatMap(lintFile), ...lintSections(), ...lintTitles()].sort();

// Fail closed: a registered mirror that no page matched means the page was
// renamed or moved and its drift gate silently stopped running.
for (const rel of MIRRORED_PAGES.keys()) {
  if (!MIRRORS_SEEN.has(rel)) {
    throw new Error(
      `${rel} is registered in MIRRORED_PAGES but no such page exists — ` +
        'point the entry at its new path, or drop it and say why the mirror no longer needs checking',
    );
  }
}

const update = process.argv.includes('--update-baseline');
if (update) {
  writeFileSync(BASELINE, JSON.stringify(current, null, 2) + '\n');
  console.log(`Baseline updated: ${current.length} tolerated violations recorded.`);
  process.exit(0);
}

let baseline = [];
try { baseline = JSON.parse(readFileSync(BASELINE, 'utf8')); } catch { baseline = []; }
const baseSet = new Set(baseline);
const fresh = current.filter((x) => !baseSet.has(x));

if (fresh.length) {
  console.error(`\n✖ ${fresh.length} new docs-style violation(s) (not in baseline):\n`);
  for (const x of fresh) {
    const [file, rule, detail] = x.split('::');
    console.error(`  ${file}  [${rule}]  ${detail}`);
  }
  console.error(`\nFix them, or (if intentional) run: npm run lint:docs -- --update-baseline\n`);
  process.exit(1);
}

const stillBaselined = current.length;
console.log(`✔ No new violations. (${stillBaselined} pre-existing violations still baselined; clear them to shrink the baseline toward zero.)`);
