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
