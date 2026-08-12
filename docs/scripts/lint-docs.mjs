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

import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';
import {
  CONTENT_ROOT, TIERS, TIER_LABELS, TIER_THRESHOLD, UNTIERED_SECTIONS, sections,
} from '../src/sidebar.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS_ROOT = join(HERE, '..');
// The same root the sidebar reads, so the two never disagree about what a page is.
const CONTENT = CONTENT_ROOT;
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
];

/// A file in the repo the docs live in, by repo-relative path. Every code-truth
/// check that derives its rule from the framework rather than restating it goes
/// through here, so the tree layout is spelled once.
function frameworkSource(rel) {
  return readFileSync(join(DOCS_ROOT, '..', ...rel.split('/')), 'utf8');
}

/// The architecture rules, as the CLI embeds them into every generated
/// project's `AGENTS.md` (and as `.claude/rules/` symlinks them).
const ARCHITECTURE_CANON = 'crates/nest-rs-cli/src/templates/architecture.md';

/// Pages that restate a file the framework ships, keyed by rel like
/// [`CONFIG_TABLES`]. Registering here is what makes the mirror *checked*, and
/// the run asserts every entry was actually visited — rename or move the page
/// and the build fails rather than the gate quietly ceasing to run.
const MIRRORED_PAGES = new Map([['architecture.mdx', (src) => architectureDrift(src)]]);
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

/// A queue is named by its `QueueName` **type**, never a string: the macro
/// rejects `#[process(queue = "audio")]` outright, and the producer's
/// string-taking `push(name, job)` is the runtime-name escape hatch, not the
/// default. Both spellings shipped in 1.1.1 across ~10 places, on pages that
/// predated `QueueName`, so a reader following the queue section
/// wrote a consumer that would not compile and a producer that silently opted
/// out of the very check the type exists to provide. Gated rather than trusted.
const QUEUE_STRING_FORM = /#\[process\(\s*queue\s*=\s*"/g;
const QUEUE_UNTYPED_PUSH = /\.(?:of::<[^>]*>\(|push\(\s*[A-Z_]{3,}\b)/g;

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
function frameworkRules() {
  const traits = new Set();
  const decorators = new Set();
  for (const file of walk(join(DOCS_ROOT, '..', 'crates'), ['.rs'])) {
    const src = readFileSync(file, 'utf8');
    for (const m of src.matchAll(/pub trait (\w+)\s*:\s*Layer\b/g)) traits.add(m[1]);
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
  return { traits, decorators: [...decorators] };
}

const { traits: LAYER_SUBTRAITS, decorators: DECORATORS } = frameworkRules();

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
const HANDLER_SNIPPET = /#\[(?:get|post|put|patch|delete)\(|#\[(?:query|mutation)\]/;

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

function frontmatter(src) {
  const m = src.match(/^---\n([\s\S]*?)\n---/);
  return m ? m[1] : null;
}

function lintFile(absPath) {
  const rel = relative(CONTENT, absPath).split('\\').join('/');
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
    for (const detail of mirror(src)) add('architecture-drift', detail);
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

const files = walk(CONTENT).sort();
const current = [...files.flatMap(lintFile), ...lintSections()].sort();

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
