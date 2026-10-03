#!/usr/bin/env node
// Docs linter — checks every page against the facts it states.
//
//   node scripts/lint-docs.mjs   # fail on any violation — there is no baseline of tolerated ones
//
// A rule earns its place by catching something a reader would act on and get wrong: a snippet
// that does not compile or no longer matches `demo/`, an install line that installs the wrong
// thing, a version pin, a figure, a dead link, frontmatter the site build needs, a member of a
// family no page names. House style — headings, word lists, asides, page shape — is review's,
// against `STYLE.md`, and never a rule here.
//
// Every framework fact a rule needs is read from the canon, which `nest-rs-conformance`'s
// `canon` binary derives from the tree and this file runs on start — see the `CANON` doc
// below. The rules quoting `demo/` read its files directly, and `readme-install` reads the
// crate READMEs: raw content, never a second derivation of a framework fact.

import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join, relative } from 'node:path';
import { REDIRECTS } from '../src/redirects.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS_ROOT = join(HERE, '..');

const REPO_ROOT = join(DOCS_ROOT, '..');

/// The docs collection root — the one place the content path is spelled.
export const CONTENT_ROOT = join(DOCS_ROOT, 'src', 'content', 'docs');

/// Every fact about the framework a page is checked against, printed by
/// `nest-rs-conformance`'s `canon` binary, which this file runs on start.
///
/// **One derivation, and no artefact.** The facts are derived once, in Rust,
/// with `syn` and `toml_edit`; this file reads them and never re-derives —
/// seven checks once re-derived them here with regexes, and the landing came to
/// claim 28 capabilities against a linter that counted 27. They used to travel
/// as a committed `docs/canon.json` that a test rewrote and failed on until it
/// was committed: half of that file's commits only bumped the test count, and
/// parallel branches collided on it. Running the generator instead means the
/// facts are as old as the tree, so no stale copy can pass and nothing waits to
/// be committed. The cost is a Rust toolchain wherever the gate runs, which the
/// docs workflow installs.
export const CANON = loadCanon();

function loadCanon() {
  let printed;
  try {
    printed = execFileSync('cargo', [
      'run', '--quiet', '--locked', '--manifest-path', join(REPO_ROOT, 'Cargo.toml'),
      '-p', 'nest-rs-conformance', '--bin', 'canon',
    ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 64 * 1024 * 1024 });
  } catch (cause) {
    throw new Error(`the canon generator failed (${cause.message}) — every code-truth check `
      + 'reads it; run `cargo run -p nest-rs-conformance --bin canon` to see why');
  }
  const parsed = JSON.parse(printed);
  // Fail closed on a canon missing a key: it would silently disable whichever
  // checks read it.
  const owed = [
    'capabilities', 'capability_crates', 'decorators', 'layer_subtraits', 'trait_methods',
    'test_count', 'version_req', 'otel_binding', 'envelope_keys', 'configs', 'architecture',
    'units', 'targets', 'queue_capabilities',
  ];
  const missing = owed.filter((key) => parsed[key] === undefined);
  if (missing.length) {
    throw new Error(`the canon is missing ${missing.join(', ')} — the generator and this file `
      + 'disagree about what it carries');
  }
  return parsed;
}

// Every page, walked once. Two consumers — the per-file lint pass, and the
// `N+ pages` floor on `/why/`, which is *checked against this number*: a second
// walk would be a second answer to the question the claim is gated on.
const PAGES = walk(CONTENT_ROOT).sort();

/// Every rule this linter can file, spelled once.
///
/// **A rule name is a string the tooling interprets, so it is a constant**: a
/// typo'd name at a call site is a violation no document describes.
///
/// The object is the family's derived population: `lint.test.mjs` joins it
/// against the fixtures that prove each rule still fires **and** against
/// `STYLE.md` § F, which describes them, so a rule cannot be added silently,
/// lost silently, or documented without existing.
export const RULES = Object.freeze({
  // The page against the site build and the site's own routes.
  frontmatter: 'frontmatter',
  description: 'description',
  link: 'link',
  // The page against the code — these read the canon, or the demo sources a fence quotes.
  versionPin: 'version-pin',
  bindOrder: 'bind-order',
  queueName: 'queue-name',
  unauthedCurl: 'unauthed-curl',
  crudError: 'crud-error',
  otelGuard: 'otel-guard',
  installStanza: 'install-stanza',
  decoratorImport: 'decorator-import',
  decoratorIndex: 'decorator-index',
  layerImpl: 'layer-impl',
  traitSurface: 'trait-surface',
  exceptionResponseError: 'exception-response-error',
  configTable: 'config-table',
  fenceDrift: 'fence-drift',
  architectureDrift: 'architecture-drift',
  envelopeDrift: 'envelope-drift',
  landingClaim: 'landing-claim',
  // Corpus-scoped — every member of a family the canon publishes is owed
  // somewhere, rather than one page owing anything.
  familyMention: 'family-mention',
  readmeInstall: 'readme-install',
});

/// Pages that restate a file the framework ships, keyed by rel like
/// [`CONFIG_TABLES`]. Registering here is what makes the mirror *checked*, and
/// the run asserts every entry was actually visited — rename or move the page
/// and the build fails rather than the gate quietly ceasing to run.
/// The value carries the rule the violations file under, so a second mirror does
/// not have to borrow the first's name.
const MIRRORED_PAGES = new Map([
  ['architecture.mdx', { rule: RULES.architectureDrift, check: (src) => architectureDrift(src) }],
  ['decorators.mdx', { rule: RULES.decoratorIndex, check: (src) => decoratorIndexDrift(src) }],
  ['index.mdx', { rule: RULES.landingClaim, check: (src) => landingClaims(src) }],
  ['why.mdx', { rule: RULES.landingClaim, check: (src) => thesisClaims(src) }],
  ['queue/writing-a-driver.mdx', { rule: RULES.envelopeDrift, check: (src) => envelopeDrift(src) }],
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
  return block ? reservedWordsIn(block.body) : null;
}

/// The token grammar itself, so the page side and the canon side cannot learn
/// different ideas of what a reserved word is. The canon arrives as the block's
/// raw text — the join slices the document and deliberately extracts nothing —
/// so this is the one place either side is tokenized.
function reservedWordsIn(body) {
  return new Set(body.split(/\s+/).filter(Boolean));
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
/// the port actually seals into a `nest_rs_queue::Envelope` rather than trusted. A key the
/// framework adds and the page omits is a driver that compiles, runs, and drops
/// that key across the one hop the framework crosses as a *process*.
function envelopeDrift(src) {
  return CANON.envelope_keys
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

/// Every decorator `/decorators/` tabulates, counted the way a reader would: the
/// distinct `#[name]` tokens in its first column. That page is itself gated
/// against `crates/*-macros/` by [`decoratorIndexDrift`], so a figure checked
/// against it is checked against the source one hop away — and the inert
/// attributes an orchestrator reads (`#[get]`, `#[public]`) count here exactly
/// because the page's inclusion rule is "everything you write".
function documentedDecorators() {
  const src = readFileSync(join(CONTENT_ROOT, 'decorators.mdx'), 'utf8');
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

/// **A page's surface is its source plus the components it renders.** The
/// landing is MDX that imports `src/components/*.astro`, and half its figures
/// are inside those components — read the MDX alone and the gate reports a
/// claim the reader can see on the page. Still `docs/**` exactly, so the
/// workflow's `paths:` filter stays a true declaration.
function surfaceOf(src) {
  const parts = [src];
  for (const m of src.matchAll(/from '(?:\.\.\/)+components\/([\w.-]+\.astro)'/g)) {
    const file = join(DOCS_ROOT, 'src', 'components', m[1]);
    if (existsSync(file)) parts.push(readFileSync(file, 'utf8'));
  }
  return parts.join('\n');
}

/// One figure, checked against the repo.
///
/// An **exact** claim (`28 capabilities`) names a set the reader can enumerate
/// on another page of this site, so any drift is a contradiction. A **floor**
/// (`2,000+ tests`) is false only once the repo holds fewer — a floor the repo
/// has outgrown is still true, so it is left alone.
///
/// A figure the pattern no longer finds is reported rather than skipped: a
/// rewording would otherwise retire the check in silence.
///
/// The emphasis is optional because a figure is not always markdown: the
/// decorator count is a sentence inside a component, where `**` would render
/// as two asterisks.
function figureDrift(surface, where, specs) {
  const out = [];
  for (const [what, shape, actual] of specs) {
    const floor = shape === 'floor';
    const re = floor
      ? new RegExp(`(?:\\*\\*)?(\\d[\\d,]*)\\+ ${what}`)
      : new RegExp(`(?:\\*\\*)?(\\d[\\d,]*) ${what}`);
    const m = surface.match(re);
    const claimed = m ? Number(m[1].replace(/,/g, '')) : null;
    if (claimed === null) {
      out.push(`no \`${floor ? 'N+' : 'N'} ${what}\` claim on ${where} — the figure is gated `
        + `against the repo, so removing it removes the gate; say what the repo holds (${actual})`);
    } else if (floor ? claimed > actual : claimed !== actual) {
      out.push(`${where} claims ${claimed}${floor ? '+' : ''} ${what}, the repo holds ${actual}`);
    }
  }
  return out;
}

/// The landing sells the framework on what a reader can go and enumerate: the
/// capability count under the capability grid, the decorator count in the
/// guarantee that names them.
function landingClaims(src) {
  return figureDrift(surfaceOf(src), 'the landing', [
    ['capabilities', 'exact', CANON.capabilities.length],
    ['decorators', 'exact', documentedDecorators()],
  ]);
}

/// `/why/` argues that the framework holds its shape over years, so the two
/// figures that measure *that* are gated where the argument is made.
///
/// They sat on the landing until 6.1, and moved because the redesigned splash
/// states neither — a figure with no home on a page is a gate with no subject,
/// and the honest repair is to gate it where the site does make the claim, not
/// to delete the check.
function thesisClaims(src) {
  return figureDrift(surfaceOf(src), '/why/', [
    ['tests', 'floor', TEST_COUNT],
    ['pages', 'floor', PAGES.length],
  ]);
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
  const out = [];

  // Both sides through one extractor: the canon arrives as the rules file's own
  // table rows, never as a token list, so "what a role token is" cannot come to
  // mean two things across the two languages.
  const canonRoles = roleTokens(CANON.architecture.role_rows.join('\n'));
  const pageRoles = roleTokens(src);
  if (pageRoles.size === 0) out.push('role table missing from the page');
  else out.push(...setDrift(canonRoles, pageRoles, 'role'));

  const canonWords = reservedWordsIn(CANON.architecture.reserved_block);
  const pageWords = reservedWords(src);
  if (!pageWords) out.push('reserved-vocabulary block missing from the page');
  else out.push(...setDrift(canonWords, pageWords, 'reserved word'));

  return out;
}

/// `major.minor` of the framework the repo currently builds — what every
/// documented `nest-rs*` pin has to say, and what `nestrs g resource` writes
/// into a generated manifest.
const VERSION_REQ = CANON.version_req;

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

/// A queue is named by its `Queue` **type**, never a string — the macro
/// refuses both string spellings by name, so the regex catches both:
/// `#[process(queue = "audio")]` shipped in 1.1.1 across ~10 places on pages
/// that predated the typed queue, and the *positional*
/// `#[process("posts.publish")]` survived that fix on a page outside the queue
/// section. A reader following either wrote a consumer that would not compile.
/// Gated rather than trusted.
const QUEUE_STRING_FORM = /#\[process\(\s*(?:queue\s*=\s*)?"/g;

/// The producer half of the same rule. `push(Q, job, options)` takes the
/// queue's `#[queue]` marker, which carries its name and its payload type, so a
/// `push` handed an ALL_CAPS name constant does not compile, and `of::<…>(…)`
/// names a surface that no longer exists: a page teaching either hands the
/// reader a push that fails to build. The one push that takes a name is
/// `push_json(name, value, options)`, the hatch for a queue this binary does not
/// declare, and the pattern leaves it alone — `.push(` never starts `push_json(`.
/// A string literal handed to `push` is not matched either: it does not compile
/// in 7.0 any more than a constant does, but it is 6.x's `push(name, job)`, and
/// the upgrade pages spell that in their before column on purpose.
const QUEUE_UNTYPED_PUSH = /\.(?:of::<[^>]*>\(|push\(\s*[A-Z_]{3,}\b)/g;

/// The words a title uses to say it is quoting the demo workspace — and the
/// whole basis on which this file resolves one.
///
/// **The claim is the marker, never the path.** `src/posts/entity.rs` says
/// where the file goes in *your* project and asserts nothing about ours, so a
/// page may illustrate it freely; the same title plus `(from the demo)` says
/// the block is an excerpt of the Publish workspace, and that is what gets
/// checked. Keying on the prefix instead made one string mean two things — the
/// reader's layout and our provenance — and a page then showed the same file at
/// two roots with nothing saying why.
const DEMO_MARKER = /\((from the demo)(?:\s*[,—][^)]*)?\)\s*$/;

/// The one prefix a feature file does not carry is `crates/features/`: it is
/// the same on every one, so the title drops it and this map puts it back. A
/// prefix that *varies* — the app, the workspace crate — stays in the title,
/// because there it is information.
const DEMO_TITLE_PREFIXES = [
  [/^src\//, 'demo/crates/features/src/'],
  [/^crates\//, 'demo/crates/'],
  [/^apps\//, 'demo/apps/'],
];

/// The repo-relative demo file a marked title names, or `null` when its path is
/// in none of the workspace shapes above.
function demoPathFor(title) {
  const clean = title.replace(DEMO_MARKER, '').trim();
  for (const [re, prefix] of DEMO_TITLE_PREFIXES) {
    if (re.test(clean)) return clean.replace(re, prefix);
  }
  return null;
}

/// A demo file's lines as an excerpt is compared against them — trimmed,
/// because an excerpt is routinely re-indented out of its `impl` block, and
/// blanks dropped, because their placement is the excerpt's — or `null` when no
/// such file exists. Read from the file itself: content is not a derivation, so
/// there is nothing to drift.
const DEMO_LINES = new Map();
function demoLines(rel) {
  if (!DEMO_LINES.has(rel)) {
    const abs = join(REPO_ROOT, rel);
    DEMO_LINES.set(rel, existsSync(abs) && statSync(abs).isFile()
      ? readFileSync(abs, 'utf8').split('\n').map((line) => line.trim()).filter(Boolean)
      : null);
  }
  return DEMO_LINES.get(rel);
}

/// Lines of a fence that claim nothing about the file — an elision mark, and
/// the fence's own closing brace where an excerpt stops mid-block.
const ELISION = /^(?:\/\/|#)\s*[….]|^\.{3}$|^…$/;

/// Whether a fence marked `(from the demo)` is an excerpt of the file it names.
///
/// **The file exists, and every non-elided line appears in it, in order.** That
/// is the claim the marker makes, and it is weaker than STYLE.md § C's
/// byte-for-byte rule on purpose: an excerpt may elide (`// …`) and re-indent.
/// It catches every way an excerpt goes stale — a line the demo rewrote, a
/// comment the demo does not carry (it carries none), a port the app does not
/// listen on, a file that moved or an app the demo never had — and the class it
/// was written for: a fence titled `mod.rs` publishing a `#[module]` that lives
/// in `module.rs`, and one titled `tests/e2e/main.rs` publishing a
/// `#[tokio::test]`. A block that is an illustration rather than an excerpt
/// drops the marker, and then asserts nothing about `demo/`.
function fenceDrift(blocks) {
  const out = [];
  for (const block of blocks) {
    const title = block.info.match(/title="([^"]+)"/)?.[1];
    if (!title || !DEMO_MARKER.test(title)) continue;
    const rel = demoPathFor(title);
    const lines = rel && demoLines(rel);
    if (!lines) {
      out.push(`${title} says it is from the demo, which has no ${rel ?? 'such path'} — either the `
        + 'file moved and the title did not, or the block is an illustration and the marker is a '
        + 'claim it cannot keep');
      continue;
    }

    let at = 0;
    for (const raw of block.body.split('\n')) {
      const line = raw.trim();
      if (!line || ELISION.test(line)) continue;
      const found = lines.indexOf(line, at);
      if (found === -1) {
        out.push(`${title} shows \`${line.slice(0, 60)}\` and ${rel} `
          + `${lines.includes(line) ? 'has it earlier — the excerpt is out of order'
            : 'does not contain that line'}`);
        break;
      }
      at = found + 1;
    }
  }
  return out;
}

/// The binding the crate's own panic text tells a reader to write when
/// `OpenTelemetryModule` was imported without `OpenTelemetry::init`. Read out of
/// the panic rather than restated: 1.3.0 corrected that message to `_otel` and
/// left the page's canonical `main` on the old `_opentelemetry`, so the reader
/// who tripped the panic was sent to a line the example he started from did not
/// contain.
const OTEL_BINDING = CANON.otel_binding;

/// The `#[config]` structs whose page publishes a key table, keyed by page and
/// carrying the source that owns the fields. A table read as exhaustive — every
/// one of these pages says so above it — that omits a field publishes a key the
/// reader has no way to learn about. `/storage/` shipped 2.0.0 listing five of
/// `StorageConfig`'s seven fields, and the missing `ALLOW_HTTP` is the one that
/// decides a boot refusal.
///
/// Which *page* publishes a key table is a docs-side fact and stays here:
/// mentioning `NESTRS_X__…` is not publishing a table, and no grep separates the
/// two. What the struct *holds* is a framework fact and comes from the canon, so
/// only the page↔struct pairing is written by hand. Add a page when it grows one.
const CONFIG_TABLES = new Map([
  ['storage/index.mdx', 'StorageConfig'],
]);

/// The field names of a `#[config]` struct, plus whether its `defaults()` is
/// profile-dependent — the second thing `/storage/` got wrong, publishing the
/// dev branch of a profile-split default as *the* default.
function configFields(struct) {
  const facts = CANON.configs[struct];
  // Fail closed: a struct the canon does not know is a `CONFIG_TABLES` entry
  // naming a type that no longer exists, which would otherwise pass vacuously.
  if (!facts) {
    throw new Error(`\`${struct}\` is not a \`#[config]\` struct in the canon — teach `
      + '`CONFIG_TABLES` its new name, do not delete the check');
  }
  return { fields: facts.fields, profileSplit: facts.profile_split, source: facts.source };
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
  return blocks.filter((block) => SHELL_INFO.test(block.info))
    .flatMap((block) => shellLines(block.body))
    .filter((line) => /^cargo\s+add\b/.test(line))
    .map(parseCargoAdd);
}

/// One `cargo add` command line: the packages it names, the features and
/// `--no-default-features` it applies to them.
function parseCargoAdd(line) {
  const tokens = line.trim().split(/\s+/).slice(2);
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
  return { line, pkgs, features, noDefault };
}

/// Span targets that instrument the framework's own internals — the DI graph
/// and the GraphQL dataloader. An operator has no decision to make about
/// either, so no page owes them a filter directive — "no", not "not yet", which
/// is why the exception is stated here with its reason.
const INTERNAL_TARGETS = new Set(['nest_rs::container', 'nest_rs::loader']);

/// The features every `cargo add … nest-rs … --features` under a page's
/// `## Install` asks for.
function installedFeatures(src) {
  return cargoAddInvocations(fencedBlocks(src).filter((b) => b.section === 'Install'))
    .filter((inv) => inv.pkgs.some((p) => p.name === 'nest-rs'))
    .flatMap((inv) => inv.features);
}

/// **`family-mention`** — every member of a family the canon publishes is named
/// on some page, in the spelling a reader types: a unit of work
/// (`graphql.operation`, what a dashboard groups on), an operator-facing span
/// target (`nest_rs::http`, what `<PREFIX>_LOG` selects on), a queue
/// `Capability::<Variant>` (what a driver declares), and an umbrella capability
/// as a `cargo add nest-rs --features <x>` under some `## Install`.
///
/// A family grows a member in Rust, and this is what makes the docs owe it a
/// line the day it exists: two units reached 5.1 named on zero of 125 pages, and
/// a feature can ship a surface no reader can discover how to install. Config
/// env keys are not a member here: a source scan for them is blind to every key
/// read through a constant, and a check blind to a quarter of its population is
/// a false guarantee.
export function familyMentions(sources, canon = CANON) {
  const named = (spelling) => sources.some((src) => src.includes(spelling));
  const installs = new Set(sources.flatMap(installedFeatures));
  const out = [];
  const add = (detail) => out.push(`(corpus)::${RULES.familyMention}::${detail}`);
  for (const unit of canon.units) {
    if (!named(unit)) add(`unit of work \`${unit}\` is named on no page`);
  }
  for (const target of canon.targets.filter((t) => !INTERNAL_TARGETS.has(t))) {
    if (!named(target)) add(`span target \`${target}\` is named on no page`);
  }
  for (const variant of canon.queue_capabilities) {
    if (!named(`Capability::${variant}`)) {
      add(`queue capability \`Capability::${variant}\` is named on no page`);
    }
  }
  for (const feature of canon.capabilities) {
    if (!installs.has(feature)) {
      add(`capability \`${feature}\` has no \`cargo add nest-rs --features ${feature}\` under any `
        + 'page\'s `## Install`');
    }
  }
  return out;
}

/// Every README a reader can land on — the repository's and each crate's, the
/// crates.io landing pages — keyed by repo-relative path, `null` when absent.
function readmes() {
  const out = new Map([['README.md', readFileSync(join(REPO_ROOT, 'README.md'), 'utf8')]]);
  for (const name of readdirSync(join(REPO_ROOT, 'crates')).sort()) {
    const path = join(REPO_ROOT, 'crates', name, 'README.md');
    out.set(`crates/${name}/README.md`, existsSync(path) ? readFileSync(path, 'utf8') : null);
  }
  return out;
}

/// **`readme-install`** — the front door is one crate. A capability crate's own
/// README, its crates.io landing page, installs the umbrella with the feature
/// (`cargo add nest-rs --features <x>`), and no README tells a reader to
/// `cargo add` a capability sub-crate instead. Both halves, because the negative
/// alone passes on an empty corpus.
export function readmeInstalls(texts, canon = CANON) {
  const out = [];
  const adds = (text) => [...(text ?? '').replace(/\\\n\s*/g, ' ')
    .matchAll(/cargo\s+add\s[^\n`]*/g)].map((m) => parseCargoAdd(m[0]));
  for (const [krate, feature] of Object.entries(canon.capability_crates)) {
    const rel = `crates/${krate}/README.md`;
    const installs = adds(texts.get(rel)).some((inv) => inv.pkgs.some((p) => p.name === 'nest-rs')
      && inv.features.includes(feature));
    if (!installs) {
      out.push(`${rel}::${RULES.readmeInstall}::no \`cargo add nest-rs --features ${feature}\` — `
        + 'a crate\'s landing page installs the umbrella, never the crate');
    }
  }
  for (const [rel, text] of texts) {
    for (const inv of adds(text)) {
      for (const pkg of inv.pkgs.filter((p) => canon.capability_crates[p.name])) {
        out.push(`${rel}::${RULES.readmeInstall}::\`${inv.line.trim()}\` installs a sub-crate — `
          + `write \`cargo add nest-rs --features ${canon.capability_crates[pkg.name]}\``);
      }
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

/// The four facts read out of the framework's own sources — *read* from the
/// canon, rather than derived here.
///
/// `layer_subtraits` — every trait declared `: Layer`. The blanket impl a reader
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
/// crate roots. It has to be exact in both directions: a name missing hides a
/// broken snippet, a name that is not a macro flags a working one. Only
/// `#[proc_macro_attribute]` entries count — the attributes an orchestrator
/// consumes (`#[query]`, `#[get]`, `#[on_module_init]`, `#[public]`) are inert
/// tokens read by the host macro, so they resolve without an import of their own
/// and must never be demanded.

/// Every `pub trait` a **page** declares, with the method names its body holds.
///
/// **This used to generate both sides of check 20, and the asymmetry that
/// replaced it is argued rather than left to be noticed.** The canon side is now
/// `syn`, in the conformance crate; this side stays a text scan because its
/// input is a fence — a snippet, often abridged, not always parseable. Sharing
/// one generator was the right answer while both sides were text: teaching one
/// about a `where` clause and not the other would have reported invented methods
/// that were not invented.
///
/// The skew that remains is one-directional and harmless, which is why the split
/// is acceptable at all. Check 20 reports a method the **page** declares and the
/// canon does not, so a canon side that reads *more* accurately can only shrink
/// the report; it can never manufacture one. The reverse — this scan missing a
/// method the page really declares — costs a missed report, never a false one,
/// and that was already true when both sides shared a generator.
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

/// The Layer System's members, the decorators, the trait surface and the test
/// floor, read from the canon.
///
/// These four used to come from one walk of every `.rs` under `crates/` — 912
/// files, parsed with regexes, from a linter whose declared input set is
/// `docs/**`. `traitDecls` below is the page-side half of the same check and
/// stays: a page publishes a trait *in a fence*, and that snippet is docs.
///
/// `trait_methods` — every `pub trait` the framework ships, mapped to the method
/// names its body declares, so a page publishing a signature can be diffed
/// against the real one. `/fundamentals/exception-filters/` published `Filter`
/// and `ExceptionFilter` with **three** methods each — four names that exist
/// nowhere under `crates/` — then spent an Aside explaining why the four do not
/// work. A reader who wrote one got `E0407`, and the page was the only source
/// that had ever claimed the method. One direction only: a page may abridge a
/// trait, it may never invent a method.
const LAYER_SUBTRAITS = new Set(CANON.layer_subtraits);
const DECORATORS = CANON.decorators;
const FRAMEWORK_TRAITS = new Map(
  Object.entries(CANON.trait_methods).map(([name, methods]) => [name, new Set(methods)]),
);
const TEST_COUNT = CANON.test_count;

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

/// Every page under `dir`.
function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (name.endsWith('.md') || name.endsWith('.mdx')) out.push(p);
  }
  return out;
}

/// The page with its fenced blocks and inline code removed, so a link spelled
/// inside code is not read as one.
function stripCode(src) {
  return stripFences(src).replace(/`[^`\n]*`/g, '');
}

/// Fenced blocks only, leaving inline code where it is.
///
/// The distinction is load-bearing for headings: `## \`ConfigService\` API`
/// renders an id built from the whole line, so stripping the span first reads
/// the heading as `API` and reports every link to it as dead. `stripCode`'s
/// inline pass is right for links — a `](/x/)` inside backticks is a code
/// sample — and wrong for anything reading a heading's text.
function stripFences(src) {
  return src.replace(/```[\s\S]*?```/g, '');
}

/// A page's id — its path under `CONTENT_ROOT`, slash-separated on every platform.
function relOf(absPath) {
  return relative(CONTENT_ROOT, absPath).split('\\').join('/');
}

/// The route a page is served at — `/http/streaming/`, `/database/`, `/`.
///
/// Derived the way Starlight derives it (`index` names its directory, the
/// extension goes, both slashes stay) rather than read back from a build, so
/// the rule runs without one. No page declares a frontmatter `slug:`; the day
/// one does, this is where it is taught.
function routeOf(rel) {
  const base = rel.replace(/\.mdx?$/, '');
  const slug = base === 'index' ? '' : base.replace(/\/index$/, '');
  return `/${slug}${slug ? '/' : ''}`;
}

/// A heading's anchor id, by GitHub's algorithm — lowercase, punctuation
/// dropped, spaces hyphenated, a duplicate suffixed `-1`, `-2`.
///
/// **Implemented rather than imported, and the reason is a rule.** Starlight
/// derives these with `github-slugger`, whose last release is 2023-09-15 —
/// outside `CLAUDE.md`'s twelve-month freshness bar, which says a failing
/// candidate is "flagged explicitly, never adopted silently". Reaching for it as
/// Astro's transitive dependency would be the same adoption without the
/// declaration. So the algorithm lives here, and `lint.test.mjs` pins the cases
/// that decide it — including the one this rule exists to catch, a heading whose
/// id nobody would guess.
function slugify(text, seen) {
  // Everything that is not a letter, number, mark, space, `-` or `_` goes.
  // `github-slugger` ships that as a ~10 KB generated character class; this is
  // the same set stated in Unicode properties, and it is verified — 934 of 934
  // anchors across the built site agree, the arrow in `User info → Principal`
  // included, which is the one a hand-written ASCII class gets wrong.
  const base = text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '')
    .replace(/\s/g, '-');
  const count = seen.get(base) ?? 0;
  seen.set(base, count + 1);
  return count ? `${base}-${count}` : base;
}

/// Every anchor a page offers, in document order.
///
/// Headings only — Starlight generates an id for each and for nothing else, so
/// an anchor pointing at a table row or a term is broken however it reads. Run
/// over the code-stripped source: a `## ` inside a fence is a shell comment.
function anchorsOf(src) {
  const seen = new Map();
  return new Set(
    [...stripFences(src).matchAll(/^#{2,6}\s+(.+?)\s*$/gm)]
      .map((m) => slugify(m[1], seen)),
  );
}

/// Every route the site serves, plus every route it redirects.
///
/// A redirect *is* resolution — that is what it is for — so `/throttler/`
/// stays a valid link target after the page moved to `/rate-limiting/`.
const ROUTES = new Set([...PAGES.map((p) => routeOf(relOf(p))), ...Object.keys(REDIRECTS)]);

/// Anchors per route, built once. 969 links across 125 pages resolve against
/// this, and re-reading a target page per link would read some of them 18 times.
const ANCHORS = new Map(
  PAGES.map((p) => [routeOf(relOf(p)), anchorsOf(readFileSync(p, 'utf8'))]),
);

/// An internal markdown link: `](/route/)`, `](/route/#anchor)`, `](#anchor)`.
/// External targets, `mailto:` and relative paths are somebody else's to check.
const INTERNAL_LINK = /\]\((\/[A-Za-z0-9/_-]*\/?)?(#[A-Za-z0-9_-]+)?\)/g;

/// Every dead internal link on one page.
///
/// **Nothing checked these, and a dead one shipped silently.** A probe page
/// linking a route that does not exist builds clean, exits 0, and serves the
/// href verbatim; the only validated target on the whole site was a sidebar
/// `slug:`, which is ~20 of them against 969 in-page links. Starlight's own
/// answer is `starlight-links-validator`, a dependency — and a link check reads
/// the page corpus and nothing else, which is a rule, not a plugin.
function deadLinks(rel, src) {
  const out = new Set();
  const here = routeOf(rel);
  for (const [, route, fragment] of stripCode(src).matchAll(INTERNAL_LINK)) {
    const target = route ?? here;
    if (route && !ROUTES.has(route)) {
      out.add(`${route} is linked and no page serves it`);
      continue;
    }
    if (!fragment) continue;
    // A redirected route's anchors belong to the page it lands on.
    const landing = REDIRECTS[target] ?? target;
    const anchors = ANCHORS.get(landing);
    if (anchors && !anchors.has(fragment.slice(1))) {
      out.add(`${target}${fragment} is linked and ${landing} has no such heading`);
    }
  }
  return out;
}

function frontmatter(src) {
  const m = src.match(/^---\n([\s\S]*?)\n---/);
  return m ? m[1] : null;
}

/// One page, linted.
///
/// `src` defaults to the file's own contents and is injectable so the suite can
/// prove a rule still fires against a fixture: a rule nothing exercises goes
/// quiet the day its regex is weakened, and the run turns greener.
export function lintFile(absPath, src = readFileSync(absPath, 'utf8')) {
  const rel = relOf(absPath);
  const blocks = fencedBlocks(src);
  const v = [];
  const add = (rule, detail) => v.push(`${rel}::${rule}::${detail}`);

  // Frontmatter — Starlight's schema requires it — with a description YAML
  // reads whole: a plain scalar ends at ` #`, so the rest of the sentence never
  // reaches the meta tag, and nothing reports it.
  const fm = frontmatter(src);
  if (fm === null) {
    add(RULES.frontmatter, 'missing frontmatter');
  } else {
    const raw = fm.match(/^description:\s*(.*)$/m)?.[1].trim() ?? '';
    const quoted = /^".*"$/.test(raw) || /^'.*'$/.test(raw);
    if (!quoted && /\s#/.test(raw)) add(RULES.description, 'unquoted-hash (YAML truncation)');
  }

  // A page that restates a shipped file may not drift from it.
  const mirror = MIRRORED_PAGES.get(rel);
  if (mirror) {
    MIRRORS_SEEN.add(rel);
    for (const detail of mirror.check(src)) add(mirror.rule, detail);
  }

  // 1. `nest-rs*` pins track the version the repo builds.
  for (const m of src.matchAll(NEST_RS_PIN)) {
    const pinned = bareReq(m[1]);
    const [major, minor] = pinned.split('.');
    if (`${major}.${minor}` !== VERSION_REQ) {
      add(RULES.versionPin, `${m[0].split('=')[0].trim()} pins ${pinned}, workspace is ${VERSION_REQ}`);
    }
  }

  // 2. The by-id binder's type parameters, in the order the code declares.
  for (const m of src.matchAll(BIND_ORDER)) {
    add(RULES.bindOrder, `${m[0]}… — the action marker comes first`);
  }

  // 3. A queue is named by its `Queue` type on both sides.
  for (const m of src.matchAll(QUEUE_STRING_FORM)) {
    add(RULES.queueName, `${m[0]}…" — name the queue by its Queue type`);
  }
  for (const m of src.matchAll(QUEUE_UNTYPED_PUSH)) {
    add(RULES.queueName, `${m[0]}… — push takes the queue's marker, as in `
      + 'push(AudioQueue, job, None); a queue this binary does not declare goes through '
      + 'push_json(name, value, options)');
  }

  for (const block of blocks) {
    const shell = SHELL_INFO.test(block.info);

    // 4. A pasteable `curl` against a guarded route carries a bearer — unless
    // the block is documenting the denial itself.
    if (shell && !/\b(401|403|Unauthorized|Forbidden)\b/.test(block.body)) {
      for (const line of shellLines(block.body)) {
        if (!/\bcurl\b/.test(line) || /authorization:/i.test(line)) continue;
        const root = guardedCurlRoot(line);
        if (root) add(RULES.unauthedCurl, `/${root} without a bearer`);
      }
    }

    // 5. A handler snippet that `?`s a `CrudService` read does not compile.
    if (RUST_INFO.test(block.info) && HANDLER_SNIPPET.test(block.body)) {
      for (const line of block.body.split('\n')) {
        if (UNMAPPED_CRUD_READ.test(line) && !line.includes('map_err')) {
          add(RULES.crudError, `unmapped DbErr: ${line.trim()}`);
        }
      }
    }
  }

  // 6. Every internal link resolves to a page, and every anchor to a heading.
  for (const detail of deadLinks(rel, src)) add(RULES.link, detail);

  // 7. The OTel guard binds the name the crate's boot panic prescribes.
  for (const m of src.matchAll(OTEL_INIT)) {
    if (m[1] !== OTEL_BINDING) {
      // No `::` in the detail — the console splits a violation on it.
      add(RULES.otelGuard, `\`let ${m[1]} =\` binds the OTel guard, but the boot panic tells the `
        + `reader to write \`let ${OTEL_BINDING} =\``);
    }
  }

  // 8. Under `## Install`, the `cargo add` line and the `[dependencies]` block
  // say the same thing.
  for (const detail of installStanzaViolations(blocks)) add(RULES.installStanza, detail);

  // 9. A snippet that shows its imports imports the decorator it illustrates.
  for (const detail of missingDecoratorImports(blocks)) add(RULES.decoratorImport, detail);

  // 10. A page-defined type implementing a Layer sub-trait carries `impl Layer`.
  const rust = rustDeclarations(blocks);
  const hasLayer = rust.implementorsOf('Layer');
  for (const { trait: t, type } of rust.impls) {
    if (!LAYER_SUBTRAITS.has(t) || !rust.types.has(type) || hasLayer.has(type)) continue;
    add(RULES.layerImpl, `impl ${t} for ${type} without \`impl Layer for ${type} {}\` — `
      + `${t} is declared \`: Layer\` and there is no blanket impl`);
  }

  // 11. An `ExceptionFilter`'s exception reaches the chain as a `poem::Error`.
  const hasResponseError = rust.implementorsOf('ResponseError');
  for (const m of src.matchAll(EXCEPTION_ASSOC)) {
    const exception = m[1];
    if (!rust.types.has(exception) || hasResponseError.has(exception)) continue;
    add(RULES.exceptionResponseError, `${exception} is claimed by an ExceptionFilter but `
      + `implements no ResponseError — the filter catches by downcast off an error that is `
      + `already a poem-Error, so the handler raising it does not compile`);
  }

  // 12. A config-key table is exhaustive, and publishes both branches of a
  // profile-dependent default.
  const configStruct = CONFIG_TABLES.get(rel);
  if (configStruct) {
    const { fields, profileSplit, source } = configFields(configStruct);
    for (const field of fields) {
      const key = field.toUpperCase();
      if (!src.includes(`\`${key}\``)) {
        add(RULES.configTable, `${configStruct}.${field} has no \`${key}\` row — the `
          + `table is published as the full key list (${source})`);
      }
    }
    if (profileSplit && !src.includes('staging/production')) {
      // No `::` in the detail — the console splits a violation on it.
      add(RULES.configTable, `${configStruct}'s defaults() branches on the profile, but `
        + 'the page never names staging/production — it publishes the dev branch as the default');
    }
  }

  // 13. A fence marked as quoting the demo is an excerpt of the file it names.
  for (const detail of fenceDrift(blocks)) add(RULES.fenceDrift, detail);

  // 14. A published trait signature does not invent a method.
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
        add(RULES.traitSurface, `${trait}-${method} is published on the page but the trait under `
          + 'crates/ declares no such method — writing it is an E0407');
      }
    }
  }

  return v;
}

/// The whole corpus, linted. Exported so the suite can assert a rule still
/// fires without running the gate — see the driver guard below.
export function lint() {
  MIRRORS_SEEN.clear();
  // `(page) => …` and not a bare reference: `flatMap` passes the index second,
  // which `lintFile`'s injectable `src` would take as the page's contents.
  const current = [
    ...PAGES.flatMap((page) => lintFile(page)),
    ...familyMentions(PAGES.map((page) => readFileSync(page, 'utf8'))),
    ...readmeInstalls(readmes()),
  ].sort();

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
  return current;
}

/// Below this floor the walk is reading the wrong tree, and a clean run proves
/// nothing: rename a section directory and its pages leave `PAGES`, every rule
/// over them stops running, and the gate would report success.
const PAGE_FLOOR = 100;

/// Run the gate. Only reached when this file is the process entry point, so
/// `import`ing it costs a canon read and a docs walk — never an `exit`.
///
/// **No baseline.** Every rule here is a fact a page contradicts, so a
/// violation is fixed on the page or the rule is wrong; a list of tolerated
/// ones would be a list of pages known to mislead their reader.
function main() {
  if (PAGES.length < PAGE_FLOOR) {
    console.error(`\n✖ the walk found ${PAGES.length} pages — below ${PAGE_FLOOR} it is reading `
      + 'the wrong tree, and a clean run proves nothing.\n');
    process.exit(1);
  }

  const violations = lint();
  if (violations.length) {
    console.error(`\n✖ ${violations.length} docs violation(s):\n`);
    for (const x of violations) {
      // A detail may quote a path, so only the first two separators are fields.
      const [file, rule, ...detail] = x.split('::');
      console.error(`  ${file}  [${rule}]  ${detail.join('::')}`);
    }
    console.error('\nEach is something the page states that the code, the demo or the site '
      + 'contradicts (STYLE.md § F). Fix the page, or the rule if it is wrong.\n');
    process.exit(1);
  }

  console.log(`✔ No violations across ${PAGES.length} pages.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
