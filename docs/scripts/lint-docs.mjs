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
// ≤3 Asides per page, example-canon ban list.
// Plus three code-truth checks the prose rules can't see — `version-pin`, `unauthed-curl`,
// `crud-error` — each documented on its constant below and filed as a 1.1.0 defect first.

import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS_ROOT = join(HERE, '..');
const CONTENT = join(DOCS_ROOT, 'src', 'content', 'docs');
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

/// `major.minor` of the framework the repo currently builds — what every
/// documented `nest-rs*` pin has to say, and what `nestrs g resource` writes
/// into a generated manifest.
function workspaceVersionReq() {
  const manifest = join(DOCS_ROOT, '..', 'Cargo.toml');
  const m = readFileSync(manifest, 'utf8')
    .match(/^\[workspace\.package\]$[\s\S]*?^version\s*=\s*"(\d+\.\d+)\./m);
  if (!m) throw new Error(`no [workspace.package] version in ${manifest}`);
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
/// teaches the wrong rule to every page that repeats it. Filed as a 1.1.1
/// defect (G5) across ~10 pages, so the shape is gated rather than trusted.
/// Same defect on the proof the binder returns: `Authorized<A, E>`
/// (`nest-rs-seaorm/src/service.rs`), action first, entity second.
const BIND_ORDER =
  /\b(?:[Bb]ind(?:_required)?(?:::)?<\s*(?:S|[A-Z]\w*Service)|Authorized<\s*(?:E|[A-Z]\w*Entity))\b/g;

/// Marks a snippet as a handler — the only layer where the check above applies.
/// A **service** method returning `ServiceError` converts `DbErr` through `?`
/// legitimately, and that is where the conversion belongs: the exemplar's
/// services return the wire type, so a handler is a one-line delegation.
const HANDLER_SNIPPET = /#\[(?:get|post|put|patch|delete)\(|#\[(?:query|mutation)\]/;

function fencedBlocks(src) {
  const out = [];
  const re = /```([^\n]*)\n([\s\S]*?)```/g;
  let m;
  while ((m = re.exec(src)) !== null) out.push({ info: m[1].trim(), body: m[2] });
  return out;
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

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const s = statSync(p);
    if (s.isDirectory()) out.push(...walk(p));
    else if (name.endsWith('.md') || name.endsWith('.mdx')) out.push(p);
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
    const pinned = m[1].replace(/^[\^~=]/, '');
    const [major, minor] = pinned.split('.');
    if (`${major}.${minor}` !== VERSION_REQ) {
      add('version-pin', `${m[0].split('=')[0].trim()} pins ${pinned}, workspace is ${VERSION_REQ}`);
    }
  }

  // 8. The by-id binder's type parameters, in the order the code declares.
  for (const m of src.matchAll(BIND_ORDER)) {
    add('bind-order', `${m[0]}… — the action marker comes first`);
  }

  for (const block of fencedBlocks(src)) {
    const shell = /^(bash|sh|shell|console|zsh)\b/.test(block.info);

    // 9. A pasteable `curl` against a guarded route carries a bearer — unless
    // the block is documenting the denial itself.
    if (shell && !/\b(401|403|Unauthorized|Forbidden)\b/.test(block.body)) {
      // Fold shell line continuations so a header on the next line counts.
      for (const line of block.body.replace(/\\\n\s*/g, ' ').split('\n')) {
        if (!/\bcurl\b/.test(line) || /authorization:/i.test(line)) continue;
        const root = guardedCurlRoot(line);
        if (root) add('unauthed-curl', `/${root} without a bearer`);
      }
    }

    // 10. A handler snippet that `?`s a `CrudService` read does not compile.
    if (/^rust\b/.test(block.info) && HANDLER_SNIPPET.test(block.body)) {
      for (const line of block.body.split('\n')) {
        if (UNMAPPED_CRUD_READ.test(line) && !line.includes('map_err')) {
          add('crud-error', `unmapped DbErr: ${line.trim()}`);
        }
      }
    }
  }

  return v;
}

const files = walk(CONTENT).sort();
const current = files.flatMap(lintFile).sort();

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
