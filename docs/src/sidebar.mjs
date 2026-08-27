// The Basics / All options tier split — the partition STYLE.md §G defines.
//
// One implementation, two readers: `scripts/lint-docs.mjs` gates it and
// `src/content.config.ts` validates the key, so the threshold, the vocabulary
// and the exemption are spelled once. Ordering is deliberately *not* here — it
// stays in each page's frontmatter `sidebar.order`, which is what Starlight
// already read. A tier partitions a section; it never re-states its order.
//
// **It is not a sidebar level.** The menu is two deep — a group is a section,
// its items are that section's pages — so a tier is rendered by the section
// index's "In this section" list and nowhere else. Here because the reading it
// encodes is worth gating; in `astro.config.mjs` it was a third level.

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

/// Non-index pages a section needs before splitting it pays. Under it the one
/// undivided list is the better read: two headers over three links cost a reader
/// more than they save.
export const TIER_THRESHOLD = 5;

/// Sections that stay flat at any size. `tutorial/` is a path, not a menu — its
/// pages are steps 1..n and the order *is* the content, so a tier boundary
/// mid-sequence tells the reader something false.
export const UNTIERED_SECTIONS = new Set(['tutorial']);

const PAGE = /\.mdx?$/;

/// The two frontmatter fields a section is derived from. Parsed rather than
/// imported: this module is loaded by the Astro config, before the content
/// collection exists.
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
/// the rest in sidebar order. `tiered` is the structural fact its readers act
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
/// a section — its pages are hand-listed across several sidebar groups, so no
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
