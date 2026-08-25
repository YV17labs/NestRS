// The Basics / All options tier split — the sidebar shape STYLE.md §G defines.
//
// One implementation, two readers: `astro.config.mjs` renders it and
// `scripts/lint-docs.mjs` gates it, so the threshold, the vocabulary and the
// exemption are spelled once. Ordering is deliberately *not* here — it stays in
// each page's frontmatter `sidebar.order`, which is what Starlight already read.
// A tier partitions a section; it never re-states its order.

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));

/// The docs collection root — the one place the content path is spelled.
export const CONTENT_ROOT = join(HERE, 'content', 'docs');

/// The two tiers, in the order a section presents them. `basics` is what a
/// reader needs to ship the common case; `all-options` is everything the
/// section also supports.
export const TIERS = ['basics', 'all-options'];

export const TIER_LABELS = { basics: 'Basics', 'all-options': 'All options' };

/// Non-index pages a section needs before splitting it pays. Under it the flat
/// list is the better sidebar: two headers over three links cost a reader more
/// than they save, which is the state this whole mechanism replaces.
export const TIER_THRESHOLD = 5;

/// Sections that stay flat at any size. `tutorial/` is a path, not a menu — its
/// pages are steps 1..n and the order *is* the content, so a tier boundary
/// mid-sequence tells the reader something false.
export const UNTIERED_SECTIONS = new Set(['tutorial']);

const PAGE = /\.mdx?$/;

/// The two frontmatter fields the sidebar reads. Parsed rather than imported:
/// this module is loaded by the Astro config, before the content collection
/// exists.
function pageMeta(file) {
  // The capture keeps its trailing newline: `sidebar:` is the last key on most
  // pages, and a line-anchored read of a block whose final line has no `\n`
  // silently returns nothing — every page then sorts as if it declared no order.
  const fm = (readFileSync(file, 'utf8').match(/^---\n([\s\S]*?\n)---/) || [])[1] ?? '';
  const tier = (fm.match(/^tier:\s*(\S+)\s*$/m) || [])[1] ?? null;
  const sidebar = (fm.match(/^sidebar:\n((?:[ \t]+.*\n)*)/m) || [])[1] ?? '';
  const order = Number((sidebar.match(/^\s+order:\s*(-?[\d.]+)\s*$/m) || [])[1]);
  return { tier, order: Number.isFinite(order) ? order : Infinity };
}

/// One section: the pages of a single content directory, its `index` apart and
/// the rest in sidebar order. `tiered` is the structural fact both readers act
/// on — the threshold and the exemption are applied here and nowhere else.
function section(dir) {
  const pages = [];
  let index = null;
  for (const name of readdirSync(join(CONTENT_ROOT, dir)).sort()) {
    const file = join(CONTENT_ROOT, dir, name);
    if (!PAGE.test(name) || !statSync(file).isFile()) continue;
    const base = name.replace(PAGE, '');
    const page = {
      ...pageMeta(file),
      rel: `${dir}/${name}`,
      slug: base === 'index' ? dir : `${dir}/${base}`,
    };
    if (base === 'index') index = page;
    else pages.push(page);
  }
  pages.sort((a, b) => a.order - b.order || a.slug.localeCompare(b.slug));
  return {
    dir,
    index,
    pages,
    tiered: !UNTIERED_SECTIONS.has(dir) && pages.length >= TIER_THRESHOLD,
  };
}

/// Every directory under the docs root that holds pages. The root itself is not
/// a section — its pages are hand-listed across three sidebar groups, so no
/// single tier split describes them.
export function sections(dir = '') {
  const out = [];
  for (const name of readdirSync(join(CONTENT_ROOT, dir)).sort()) {
    if (!statSync(join(CONTENT_ROOT, dir, name)).isDirectory()) continue;
    const child = dir ? `${dir}/${name}` : name;
    out.push(section(child), ...sections(child));
  }
  return out;
}

/// One section's sidebar entries. A flat section keeps `autogenerate` — the
/// same list Starlight built before, from the same frontmatter. A tiered one
/// puts its `index` above the two groups, because the page that frames the
/// split belongs above it.
///
/// Fails the build rather than the reader. A page with no tier would otherwise
/// drop out of the sidebar entirely, and an empty group would publish a header
/// over nothing — both read as a claim about the section that nobody made.
export function sidebarSection(dir) {
  const { index, pages, tiered } = section(dir);
  if (!tiered) return [{ autogenerate: { directory: dir } }];
  if (!index) throw new Error(`${dir}/ is tiered but has no index page to sit above the groups`);

  const stray = pages.filter((p) => !TIERS.includes(p.tier));
  if (stray.length) {
    throw new Error(
      `${stray.map((p) => p.rel).join(', ')}: no tier — every page of a ${pages.length}-page ` +
        `section declares \`tier: ${TIERS.join('\` or \`tier: ')}\` (STYLE.md §G)`,
    );
  }
  const groups = TIERS.map((tier) => ({
    label: TIER_LABELS[tier],
    items: pages.filter((p) => p.tier === tier).map((p) => ({ slug: p.slug })),
  }));
  const empty = groups.filter((g) => !g.items.length);
  if (empty.length) {
    throw new Error(
      `${dir}/ declares no ${empty.map((g) => g.label).join(' and no ')} page — a section split ` +
        'into one tier is the flat list with a header on it (STYLE.md §G)',
    );
  }
  return [{ slug: index.slug }, ...groups];
}
