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
// Plus the code-truth checks the prose rules can't see — `version-pin`, `unauthed-curl`,
// `crud-error`, `bind-order`, `queue-name`, `install-stanza`, `otel-guard` — each documented on
// its constant below and filed as a shipped defect first.

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

/// A queue is named by its `QueueName` **type**, never a string: the macro
/// rejects `#[process(queue = "audio")]` outright, and the producer's
/// string-taking `push(name, job)` is the runtime-name escape hatch, not the
/// default. Both spellings shipped across ~10 places on pages that predated
/// `QueueName` (filed as 1.1.1 Q2), so a reader following the queue section
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
  const src = readFileSync(
    join(DOCS_ROOT, '..', 'crates', 'nest-rs-opentelemetry', 'src', 'module.rs'), 'utf8');
  const m = src.match(/Add `let (\w+) =/);
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
    if (/^rust\b/.test(block.info) && HANDLER_SNIPPET.test(block.body)) {
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
