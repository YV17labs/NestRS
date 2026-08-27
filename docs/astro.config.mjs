import { defineConfig } from 'astro/config';
import { unified } from '@astrojs/markdown-remark';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';
import mermaid from 'astro-mermaid';
import remarkGfm from 'remark-gfm';
import rehypeExternalLinks from 'rehype-external-links';
import { REDIRECTS } from './src/redirects.mjs';
import { DEFAULT_DESCRIPTION } from './src/brand.mjs';

// GitHub Pages: nestrs.dev (custom domain, base /). Local dev defaults match.
// CI sets ASTRO_SITE + ASTRO_BASE — see .github/workflows/docs-pages.yml.
const base = process.env.ASTRO_BASE || '/';
const site = process.env.ASTRO_SITE || 'https://nestrs.dev';
const asset = (path) => `${base}${path.replace(/^\//, '')}`;

const defaultDescription = DEFAULT_DESCRIPTION;
const ogImage = new URL(asset('social-preview.png'), site).href;
const ogImageAlt = 'NestRS — The Rust framework for modular, scalable backends';

// The code palette the redesign specifies: decorators carry the accent,
// keywords are cool, strings green, comments the faintest text. Written as a
// TextMate theme so one declaration serves every fence Expressive Code renders
// — a per-page override would be the same decision spelled twice.
const nestrsCodeTheme = {
  name: 'nestrs-dark',
  type: 'dark',
  colors: {
    'editor.background': '#1a191d',
    'editor.foreground': '#c9c4bb',
  },
  // Four colours, and everything else is prose.
  //
  // The design's code cards ink exactly four things — an attribute, a keyword,
  // a string, a comment — and leave types, function names, variables, numbers
  // and punctuation at the body colour. A highlighter's instinct is to paint
  // all of those too, which is what made these cards look busy beside the
  // mockup. The scopes below were read off Shiki's own Rust grammar rather than
  // guessed, which is why they are narrow: `keyword.operator` covers `=`, `::`,
  // `.` and `->`, so the keyword rule names its branches instead of the root.
  tokenColors: [
    {
      scope: ['comment', 'punctuation.definition.comment'],
      settings: { foreground: '#5f5b54', fontStyle: 'italic' },
    },
    {
      scope: ['string', 'string.quoted', 'constant.character', 'punctuation.definition.string'],
      settings: { foreground: '#9fc490' },
    },
    {
      scope: [
        'keyword.other',
        'keyword.control',
        'keyword.declaration',
        'storage.type',
        'storage.modifier',
      ],
      settings: { foreground: '#8ab4d8' },
    },
    {
      // The attribute, brackets and name together. The grammar gives the name
      // itself no scope of its own — it is only `meta.attribute` — so reaching
      // it means taking the wrapper, which also inks an attribute's arguments.
      // That is the closer of the two misses: a decorator whose name reads grey
      // is the thing the eye notices on these pages.
      scope: [
        'meta.attribute',
        'punctuation.definition.attribute',
        'punctuation.brackets.attribute',
        'entity.name.function.macro',
        'support.function.macro',
      ],
      settings: { foreground: '#ff6e5a' },
    },
  ],
};

export default defineConfig({
  site,
  base,
  // One entry per moved/deleted/renamed page (audit §2.8.B). No 404 for a URL
  // that ever shipped.
  // Declared in `src/redirects.mjs` so the docs linter's `link` rule reads the
  // same list: a retired route is resolvable exactly because it redirects, and
  // a second copy would make that true for one reader and false for the other.
  redirects: REDIRECTS,
  // GFM tables/strikethrough/task-lists must be enabled for .mdx. Since
  // @astrojs/mdx@7 the processor carries that — the top-level `markdown.gfm`
  // flag Astro 6 needed is deprecated and gone.
  markdown: {
    processor: unified({
      gfm: true,
      remarkPlugins: [remarkGfm],
      // External links open in a new tab (with rel="noopener noreferrer") so a
      // reader following e.g. the SeaORM link keeps the docs open. Internal links
      // are left untouched.
      rehypePlugins: [
        [rehypeExternalLinks, { target: '_blank', rel: ['noopener', 'noreferrer'] }],
      ],
    }),
  },
  integrations: [
    mermaid(),
    starlight({
      title: 'NestRS',
      description: defaultDescription,
      routeMiddleware: './src/routeData.ts',
      head: [
        { tag: 'meta', attrs: { name: 'theme-color', content: '#121114' } },
        // Schibsted Grotesk (UI/prose) and Martian Mono (code, labels, figures)
        // are the redesign's two faces. Google Fonts serves both; the preconnects
        // buy the handshake back on the first paint.
        { tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.googleapis.com' } },
        {
          tag: 'link',
          attrs: { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: true },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'stylesheet',
            href:
              'https://fonts.googleapis.com/css2?family=Schibsted+Grotesk:wght@400;500;600;700;800' +
              '&family=Martian+Mono:wght@300;400;500;700&display=swap',
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'apple-touch-icon',
            href: asset('apple-touch-icon.png'),
            sizes: '180x180',
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'icon',
            type: 'image/png',
            href: asset('apple-touch-icon.png'),
          },
        },
        { tag: 'meta', attrs: { property: 'og:image', content: ogImage } },
        { tag: 'meta', attrs: { property: 'og:image:width', content: '1280' } },
        { tag: 'meta', attrs: { property: 'og:image:height', content: '640' } },
        { tag: 'meta', attrs: { property: 'og:image:alt', content: ogImageAlt } },
        { tag: 'meta', attrs: { name: 'twitter:image', content: ogImage } },
        { tag: 'meta', attrs: { name: 'twitter:image:alt', content: ogImageAlt } },
      ],
      plugins: [
        starlightLlmsTxt({
          projectName: 'NestRS',
          description:
            'The Rust framework for modular, scalable backends: declarative, multi-transport, boot-time wiring checks, scoped data access by composition.',
          details:
            'NestRS sits on top of hyper/tokio/poem. It is decorator-driven (procedural macros: #[module], #[controller], #[resolver], #[gateway], #[processor], #[scheduled], #[mcp]), with a flat type-id DI container verified at boot (the "access graph"), an ambient data context that installs a request-scoped executor and ability, row-level filtering and response masking via ability-based authorization, and per-binary subsets through module-gated discovery. NestRS is opinionated about layout and naming, and #[module] carries no "controllers" list, so a type\'s name is the only thing that says what it is for: read /architecture/ first and follow it when writing or reviewing NestRS code. A generated project commits the same rules as AGENTS.md at its root.',
          // The plugin's own default is `['index*']`; naming any value replaces
          // it, so the index has to be restated or it loses its lead position.
          promote: ['index*', 'architecture'],
        }),
      ],
      favicon: '/favicon.svg',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/YV17labs/NestRS' },
      ],
      expressiveCode: {
        themes: [nestrsCodeTheme, 'github-light'],
        styleOverrides: {
          borderRadius: '12px',
          codePaddingBlock: '1.125rem',
          codePaddingInline: '1.375rem',
          borderColor: 'var(--nestrs-card-border)',
          codeBackground: 'var(--nestrs-code-bg)',
          codeFontFamily: 'var(--sl-font-mono)',
          uiFontFamily: 'var(--sl-font)',
          // The design sets code at 12.5px on a 1.8 measure, and that has to be
          // said here rather than in `custom.css`: Expressive Code puts the
          // text in a `pre > code` it resets with `all: unset` and then sizes
          // from `--ec-codeFontSize`, so a rule on the `<pre>` never reaches the
          // lines. Left unsaid, the block inherited Starlight's 14px/1.75 —
          // a face and a half larger than the landing's own cards, and wide
          // enough to put a horizontal scrollbar under snippets the design fits.
          codeFontSize: '0.78125rem',
          codeLineHeight: '1.8',
          // A code card is one surface with a header rule across it, not an
          // editor with a tab: the filename says which file, and a tab shape
          // would promise siblings the block does not have.
          // A marked line wears the accent wash and bar the landing's code cards
          // use, never Expressive Code's default blue.
          textMarkers: {
            markBackground: 'color-mix(in srgb, var(--nestrs-accent) 9%, transparent)',
            markBorderColor: 'var(--nestrs-accent)',
          },
          frames: {
            // The tab shape, squared off. `.title` is still Expressive Code's
            // editor tab — `overflow: hidden` and a radius of its own — and the
            // stylesheet strips its border and its padding to make it read as a
            // caption. Left rounded, that 12px arc was carved out of a box only
            // as tall as the text, so the first and last glyph of a filename
            // came out clipped: `crates/…/strategy.rs` lost the `c` and the
            // `rs`. Said here rather than in CSS because the tab is what the
            // frames plugin draws, and `borderRadius` is what it inherits from.
            // Unitless values are rejected for this key, hence `0px`.
            editorTabBorderRadius: '0px',
            editorBackground: 'var(--nestrs-code-bg)',
            terminalBackground: 'var(--nestrs-code-bg)',
            editorTabBarBackground: 'transparent',
            editorActiveTabBackground: 'transparent',
            editorActiveTabIndicatorTopColor: 'transparent',
            editorActiveTabIndicatorBottomColor: 'transparent',
            editorActiveTabBorderColor: 'transparent',
            editorTabBarBorderBottomColor: 'var(--nestrs-card-border)',
            terminalTitlebarBackground: 'transparent',
            terminalTitlebarBorderBottomColor: 'var(--nestrs-card-border)',
            terminalTitlebarDotsOpacity: '0',
            inlineButtonBorder: 'var(--nestrs-card-border)',
            frameBoxShadowCssValue: 'none',
          },
        },
      },
      customCss: ['./src/styles/custom.css'],
      components: {
        PageFrame: './src/components/PageFrame.astro',
        Hero: './src/components/Hero.astro',
        Footer: './src/components/Footer.astro',
        // The lockup's badge is a skewed plate with counter-skewed italic type
        // and a gradient-clipped wordmark. An `<img>` would render neither the
        // webfont nor the clip, so the brand is markup rather than a logo file.
        SiteTitle: './src/components/SiteTitle.astro',
        SocialIcons: './src/components/SocialIcons.astro',
        PageTitle: './src/components/PageTitle.astro',
        Sidebar: './src/components/Sidebar.astro',
      },
      editLink: {
        baseUrl: 'https://github.com/YV17labs/NestRS/edit/main/docs/',
      },
      lastUpdated: true,
      // Two levels, and never a third: a group names a section, its items are
      // that section's pages. The design's menu has that shape and ours had
      // four levels — an umbrella group holding only groups (`Transports`), the
      // section, the Basics / All options split, then the page. The split now
      // lives where it costs no level: the section index's "In this section"
      // list (STYLE.md §G), which is prose a reader is already inside.
      //
      // So a section is a top-level group here, and `autogenerate` fills it
      // from each page's frontmatter `sidebar.order` — a page added tomorrow
      // lands in its section without this file being touched. A group that
      // gathers more than one directory lists them in reading order; one whose
      // directory holds sub-directories (`security/`) lists its own pages by
      // slug, because autogenerating it would nest and that is the third level
      // again.
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Introduction', slug: 'index' },
            { label: 'Why NestRS', slug: 'why' },
            { label: 'Why not axum?', slug: 'why-not-axum' },
            { label: 'Benchmarks', slug: 'benchmarks' },
            { label: 'Coming from NestJS', slug: 'coming-from-nestjs' },
            { label: 'Getting started', slug: 'getting-started' },
            { label: 'CLI', slug: 'cli' },
            { label: 'The demo apps (Publish)', slug: 'publish' },
          ],
        },
        { label: 'Tutorial', items: [{ autogenerate: { directory: 'tutorial' } }] },
        {
          // `architecture` sits last, not first: it is the group's deep
          // reference — role tables, the five naming levels, the reserved
          // vocabulary — and putting it above the section's own Overview left
          // the overview reading as the second thing in its own section.
          label: 'Fundamentals',
          items: [
            { autogenerate: { directory: 'fundamentals' } },
            { label: 'Architecture and naming', slug: 'architecture' },
          ],
        },
        { label: 'Configuration', items: [{ autogenerate: { directory: 'configuration' } }] },
        {
          label: 'HTTP',
          items: [
            { autogenerate: { directory: 'http' } },
            { label: 'OpenAPI', slug: 'openapi' },
          ],
        },
        { label: 'GraphQL', items: [{ autogenerate: { directory: 'graphql' } }] },
        { label: 'WebSockets', items: [{ autogenerate: { directory: 'websockets' } }] },
        { label: 'MCP', items: [{ autogenerate: { directory: 'mcp' } }] },
        {
          label: 'Data',
          items: [
            { autogenerate: { directory: 'database' } },
            { label: 'File storage', slug: 'storage' },
          ],
        },
        {
          // The two recipes sit between the overview and the threat model: the
          // order a reader meets them in, and the reason this group is spelled
          // out rather than autogenerated.
          label: 'Security',
          items: [
            { label: 'Overview', slug: 'security' },
            { label: 'Add login + protect a route', slug: 'security/add-login' },
            { label: 'Multi-tenant SaaS in production', slug: 'security/multi-tenant-saas' },
            { label: 'Threat model', slug: 'security/threat-model' },
          ],
        },
        {
          label: 'Authentication',
          items: [{ autogenerate: { directory: 'security/authentication' } }],
        },
        {
          label: 'Authorization',
          items: [{ autogenerate: { directory: 'security/authorization' } }],
        },
        {
          label: 'Background work',
          items: [
            { autogenerate: { directory: 'queue' } },
            { label: 'Scheduling', slug: 'schedule' },
            { label: 'Events', slug: 'events' },
          ],
        },
        { label: 'Testing', items: [{ autogenerate: { directory: 'testing' } }] },
        {
          label: 'Operations',
          items: [
            { autogenerate: { directory: 'opentelemetry' } },
            { label: 'Server-Timing', slug: 'server-timing' },
            { autogenerate: { directory: 'health' } },
            { label: 'Rate limiting', slug: 'rate-limiting' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Packages', slug: 'packages' },
            { label: 'Decorator reference', slug: 'decorators' },
            { label: 'Glossary', slug: 'glossary' },
          ],
        },
      ],
    }),
  ],
});
