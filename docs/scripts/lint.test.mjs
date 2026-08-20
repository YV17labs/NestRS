// The linter, joined against itself.
//
// Thirty rules, each written as the fix for a named shipped defect, and until
// this file not one had a proof that it still fired. Weaken any regex and the
// run went *greener* — `.claude/rules/testing.md`: "A green cell that would stay
// green is worse than an empty one: the matrix reads as covered and the join
// goes quiet." With the baseline at zero the signal was gone entirely, since
// there was not even a count that could move.
//
// So the rules are a family and this is their join. Members are derived from
// `RULES`, never listed here; the obligations are three, and a member owing an
// empty cell fails:
//
//   1. **it fires** — a fixture that must produce it, so a rule that stops
//      working fails rather than going quiet;
//   2. **it is documented** — a `STYLE.md` § F entry, so a rule cannot ship
//      unwritten (seven had);
//   3. **nothing else fires** — every violation the real corpus produces names
//      a member, so a rule name typo'd at a call site cannot hide as a
//      violation the baseline can never match.
//
// A fixture proves the rule *triggers*, never that its judgement is right; that
// is `/audit`'s question and the two do not substitute.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

import { CONTENT_ROOT } from '../src/sidebar.mjs';
import { RULES, lintFile, lint } from './lint-docs.mjs';

/// The version a documented pin has to carry, read from the same canon the rule
/// reads — a literal here would make the `install-stanza` fixture fail on the
/// next release rather than when the rule breaks.
const CANON_VERSION = JSON.parse(
  readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', 'canon.json'), 'utf8'),
).version_req;

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS_ROOT = join(HERE, '..');

/// A well-formed page, so a fixture varies exactly one thing.
///
/// Every fixture below starts from this and breaks one rule. Without a shared
/// base a fixture trips its neighbours and the assertion that it produced *its*
/// rule passes for the wrong reason.
const OK = `---
title: Sample
description: A page that breaks nothing, so a fixture can break one thing.
---

Prose that says something.

## Going further

- [Testing](/testing/)
`;

/// Replace one line of the base page, or append when the marker is absent.
function page(...extra) {
  return OK.replace('\nProse that says something.\n', `\n${extra.join('\n')}\n`);
}

/// The rule names one page-fixture produces.
function rulesFor(rel, src) {
  return new Set(lintFile(join(CONTENT_ROOT, rel), src).map((v) => v.split('::')[1]));
}

/// One fixture per rule: the page that must produce it, and where it goes.
///
/// The rel path matters for the rules keyed on it — a mirror, a config table, a
/// tier — so each entry names the page it belongs to rather than a generic one.
const FIXTURES = [
  [RULES.frontmatter, 'sample.mdx', 'No frontmatter at all.\n'],
  [RULES.description, 'sample.mdx', '---\ntitle: Sample\n---\n\nBody.\n'],
  [RULES.heading, 'sample.mdx', page('## Next steps', '', 'Body.')],
  [RULES.bannedWord, 'sample.mdx', page('This is blazingly fast.')],
  [RULES.exclamation, 'sample.mdx', page('It works!')],
  [RULES.goingFurther, 'sample.mdx', '---\ntitle: Sample\ndescription: A page with no closing block.\n---\n\nBody.\n'],
  [RULES.asides, 'sample.mdx', page(...Array(4).fill('<Aside>Note.</Aside>'))],
  [RULES.canon, 'sample.mdx', page('The `ItemsService` resolves it.')],
  [RULES.link, 'sample.mdx', page('See [nowhere](/no-such-route/).')],
  [RULES.versionPin, 'sample.mdx', page('```toml', 'nest-rs = "0.1"', '```')],
  [RULES.bindOrder, 'sample.mdx', page('```rust', 'fn f(b: Bind<PostsService, Read>) {}', '```')],
  [RULES.queueName, 'sample.mdx', page('```rust', '#[process(queue = "audio")]', 'fn f() {}', '```')],
  [RULES.unauthedCurl, 'sample.mdx', page('```bash', 'curl http://localhost:3002/v1/posts', '```')],
  [RULES.crudError, 'sample.mdx', page(
    '```rust', '#[get("/posts")]', 'async fn all(&self) -> Result<Json<Vec<Post>>> {',
    '    let all = self.svc.list().await?;', '    Ok(Json(all))', '}', '```',
  )],
  [RULES.otelGuard, 'sample.mdx', page('```rust', 'let _wrong = OpenTelemetry::init(cfg);', '```')],
  [RULES.installStanza, 'sample.mdx', OK.replace('\n## Going further\n', `
## Install

\`\`\`bash
cargo add nest-rs --features http
\`\`\`

\`\`\`toml
[dependencies]
nest-rs = { version = "${CANON_VERSION}", features = ["http", "graphql"] }
\`\`\`

## Going further
`)],
  [RULES.decoratorImport, 'sample.mdx', page(
    '```rust', 'use nest_rs::core::Module;', '', '#[controller(path = "/x")]', 'pub struct C;', '```',
  )],
  [RULES.layerImpl, 'sample.mdx', page(
    '```rust', 'pub struct MyGuard;', '', 'impl Guard for MyGuard {}', '```',
  )],
  [RULES.traitSurface, 'sample.mdx', page(
    '```rust', 'pub trait Guard {', '    fn check_http(&self);', '    fn invented(&self);', '}',
    '```',
  )],
  [RULES.exceptionResponseError, 'sample.mdx', page(
    '```rust', 'pub enum MyError { Nope }', '', 'impl ExceptionFilter for F {',
    '    type Exception = MyError;', '}', '```',
  )],
  [RULES.bareLog, 'sample.mdx', page('```rust', 'tracing::info!("started");', '```')],
  [RULES.forRootForm, 'sample.mdx', page('```rust', 'HttpModule::for_root(Some(cfg))', '```')],
  [RULES.fenceDrift, 'sample.mdx', page(
    '```rust title="apps/api/src/module.rs"', 'pub struct NotInThatFile;', '```',
  )],
  [RULES.fenceTitle, 'sample.mdx', page(
    '```rust title="apps/api/src/module.rs"', '/// a doc comment the demo workspace forbids',
    'pub struct ApiModule;', '```',
  )],
  [RULES.testLayout, 'sample.mdx', page('| Suite | `tests/e2e.rs` |')],
  [RULES.configTable, 'storage/index.mdx', page('The keys are `ENDPOINT` and nothing else.')],
  [RULES.architectureDrift, 'architecture.mdx', page('No role table, no reserved block.')],
  [RULES.decoratorIndex, 'decorators.mdx', page('An index with no rows.')],
  [RULES.landingClaim, 'index.mdx', page('No figures at all.')],
  [RULES.envelopeDrift, 'queue/writing-a-driver.mdx', page('```json', '{}', '```')],
];

for (const [rule, rel, src] of FIXTURES) {
  test(`${rule} fires`, () => {
    const fired = rulesFor(rel, src);
    assert.ok(
      fired.has(rule),
      `the ${rule} fixture produced ${[...fired].join(', ') || 'nothing'} — either the rule `
        + 'stopped firing, or the fixture stopped breaking it. Both are the cell going quiet.',
    );
  });
}

/// The two rules whose population is the corpus, not a page: they cannot take a
/// fixture, so they are covered by what the real tree already exercises.
///
/// Stated rather than skipped. `tier` and `title` are computed by `lintSections`
/// and `lintTitles` over every page at once — a fixture would have to replace
/// the corpus, and a corpus-shaped fixture is a second walk with its own answer.
/// What is asserted instead is that both rules are *reachable*: the functions
/// run over the real tree on every gate, and obligation 3 below proves nothing
/// else can file under their names.
const CORPUS_SCOPED = new Set([RULES.tier, RULES.title]);

/// The anchor cases that decide `slugify`, pinned because the algorithm is
/// written out rather than imported — see the `link` rule's note on the
/// freshness bar.
///
/// The arrow is the case that matters: an ASCII-only punctuation class keeps it
/// and derives `user-info-→-principal`, while the build renders
/// `user-info--principal`. That single character is what a hand-rolled slugger
/// gets wrong, and the whole corpus was compared against the built site once to
/// establish the rest — 933 anchors, agreeing in both directions.
const SLUGS = [
  ['User info → Principal', 'user-info--principal'],
  ['`ConfigService` API', 'configservice-api'],
  ['What fails if you get it wrong', 'what-fails-if-you-get-it-wrong'],
  ['Advanced: hand-written `from_env`', 'advanced-hand-written-from_env'],
  ['Réglages spécifiques', 'réglages-spécifiques'],
];

for (const [heading, expected] of SLUGS) {
  test(`anchor id for "${heading}"`, () => {
    const src = page(`## ${heading}`, '', 'Body.');
    const [, , detail] = lintFile(join(CONTENT_ROOT, 'sample.mdx'),
      `${src}\n[here](#${expected})\n`).find((v) => v.includes('::link::')) ?? [];
    assert.equal(detail, undefined,
      `the link to #${expected} was reported dead, so slugify no longer derives it from `
        + `"${heading}" — the algorithm drifted from what the build renders`);
  });
}

test('a duplicate heading takes the -1 suffix', () => {
  const src = page('## Limits', '', 'One.', '', '## Limits', '', 'Two.');
  const dead = lintFile(join(CONTENT_ROOT, 'sample.mdx'), `${src}\n[a](#limits) [b](#limits-1)\n`)
    .filter((v) => v.includes('::link::'));
  assert.deepEqual(dead, [], 'a repeated heading gets `-1`, the way GitHub numbers duplicates');
});

test('every rule has a fixture that proves it still fires', () => {
  const proved = new Set(FIXTURES.map(([rule]) => rule));
  const owed = Object.values(RULES).filter(
    (rule) => !proved.has(rule) && !CORPUS_SCOPED.has(rule),
  );
  assert.deepEqual(
    owed, [],
    'these rules can be deleted or broken and every suite stays green — add a fixture to '
      + 'FIXTURES, or, if the rule is genuinely corpus-scoped, to CORPUS_SCOPED with the reason',
  );
});

test('every rule is documented in STYLE.md', () => {
  const style = readFileSync(join(DOCS_ROOT, 'STYLE.md'), 'utf8');
  const undocumented = Object.values(RULES).filter((rule) => !style.includes(`\`${rule}\``));
  assert.deepEqual(
    undocumented, [],
    'these rules gate the corpus and STYLE.md — the document it calls itself the law of — '
      + 'does not mention them. Seven shipped that way, all of them the most recent, which is '
      + 'the direction this drifts: the linter grows and the prose does not follow',
  );
});

test('no violation names a rule outside RULES', () => {
  const stray = [...new Set(lint().map((v) => v.split('::')[1]))]
    .filter((rule) => !Object.values(RULES).includes(rule));
  assert.deepEqual(
    stray, [],
    'a rule name reached a violation without going through RULES — a literal at a call site, '
      + 'which is a name the baseline can never match and no document describes',
  );
});

test('every fixture rule name is one RULES holds', () => {
  const stray = FIXTURES.map(([rule]) => rule).filter((r) => !Object.values(RULES).includes(r));
  assert.deepEqual(stray, [], 'a fixture names a rule that does not exist');
});
