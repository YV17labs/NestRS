import { defineConfig } from 'astro/config';
import { unified } from '@astrojs/markdown-remark';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';
import mermaid from 'astro-mermaid';
import remarkGfm from 'remark-gfm';
import rehypeExternalLinks from 'rehype-external-links';
import { sidebarSection } from './src/sidebar.mjs';

// GitHub Pages: nestrs.dev (custom domain, base /). Local dev defaults match.
// CI sets ASTRO_SITE + ASTRO_BASE — see .github/workflows/docs-pages.yml.
const base = process.env.ASTRO_BASE || '/';
const site = process.env.ASTRO_SITE || 'https://nestrs.dev';
const asset = (path) => `${base}${path.replace(/^\//, '')}`;

const defaultDescription =
  'Scalable Rust backend apps with native performance.';
const ogImage = new URL(asset('social-preview.png'), site).href;
const ogImageAlt =
  'NestRS — Scalable Rust backend apps with native performance';

export default defineConfig({
  site,
  base,
  // One entry per moved/deleted/renamed page (audit §2.8.B). No 404 for a URL
  // that ever shipped.
  redirects: {
    '/graphql/dataloader/': '/database/dataloaders/',
    '/throttler/': '/rate-limiting/',
  },
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
        { tag: 'meta', attrs: { name: 'theme-color', content: '#161619' } },
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
            'Scalable Rust backend apps with native performance — declarative framework, multi-transport, boot-time wiring checks, scoped data access by composition.',
          details:
            'NestRS sits on top of hyper/tokio/poem. It is decorator-driven (procedural macros: #[module], #[controller], #[resolver], #[gateway], #[processor], #[scheduled], #[mcp]), with a flat type-id DI container verified at boot (the "access graph"), an ambient data context that installs a request-scoped executor and ability, row-level filtering and response masking via ability-based authorization, and per-binary subsets through module-gated discovery. NestRS is opinionated about layout and naming, and #[module] carries no "controllers" list, so a type\'s name is the only thing that says what it is for: read /architecture/ first and follow it when writing or reviewing NestRS code. A generated project commits the same rules as AGENTS.md at its root.',
          // The plugin's own default is `['index*']`; naming any value replaces
          // it, so the index has to be restated or it loses its lead position.
          promote: ['index*', 'architecture'],
        }),
      ],
      logo: {
        light: './src/assets/logo.svg',
        dark: './src/assets/logo.svg',
        replacesTitle: true,
      },
      favicon: '/favicon.svg',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/YV17labs/NestRS' },
      ],
      expressiveCode: {
        themes: ['one-dark-pro', 'github-light'],
        styleOverrides: {
          borderRadius: '12px',
          borderColor: 'var(--nestrs-card-border)',
          codeBackground: 'var(--nestrs-code-bg)',
          codeFontFamily: 'var(--sl-font-mono)',
          uiFontFamily: 'var(--sl-font)',
          frames: {
            editorBackground: 'var(--nestrs-code-bg)',
            terminalBackground: 'var(--nestrs-code-bg)',
            editorTabBarBackground: 'var(--nestrs-code-tabbar)',
            editorActiveTabBackground: 'var(--nestrs-code-bg)',
            editorActiveTabIndicatorTopColor: 'transparent',
            editorActiveTabIndicatorBottomColor: 'var(--nestrs-orange)',
            editorActiveTabBorderColor: 'var(--nestrs-card-border)',
            editorTabBarBorderBottomColor: 'var(--nestrs-card-border)',
            terminalTitlebarBackground: 'var(--nestrs-code-tabbar)',
            terminalTitlebarBorderBottomColor: 'var(--nestrs-card-border)',
            frameBoxShadowCssValue: '0 16px 40px -16px rgba(0, 0, 0, 0.55)',
          },
        },
      },
      customCss: ['./src/styles/custom.css'],
      components: {
        PageFrame: './src/components/PageFrame.astro',
        Hero: './src/components/Hero.astro',
        Footer: './src/components/Footer.astro',
      },
      editLink: {
        baseUrl: 'https://github.com/YV17labs/NestRS/edit/main/docs/',
      },
      lastUpdated: true,
      // Nine doors (audit §2.4.11): a newcomer reads the group labels as a path.
      // Within-section order is per-page frontmatter `sidebar.order`, and
      // `sidebarSection` splits a section into Basics / All options from each
      // page's frontmatter `tier` (STYLE.md §G) — so a page added tomorrow lands
      // in a tier without this file being touched. Sections under the split's
      // threshold keep Starlight's plain `autogenerate`.
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
        { label: 'Tutorial', items: sidebarSection('tutorial') },
        {
          label: 'Concepts',
          items: [
            { label: 'Architecture and naming', slug: 'architecture' },
            { label: 'Fundamentals', items: sidebarSection('fundamentals') },
            { label: 'Configuration', items: sidebarSection('configuration') },
          ],
        },
        {
          label: 'Transports',
          items: [
            { label: 'HTTP', items: sidebarSection('http') },
            { label: 'GraphQL', items: sidebarSection('graphql') },
            { label: 'WebSockets', items: sidebarSection('websockets') },
            { label: 'MCP', items: sidebarSection('mcp') },
            { label: 'OpenAPI', items: sidebarSection('openapi') },
          ],
        },
        {
          label: 'Data',
          items: [
            { label: 'Database', items: sidebarSection('database') },
            { label: 'File storage', items: sidebarSection('storage') },
          ],
        },
        {
          label: 'Security',
          items: [
            { label: 'Overview', slug: 'security' },
            { label: 'Add login + protect a route', slug: 'security/add-login' },
            { label: 'Multi-tenant SaaS in production', slug: 'security/multi-tenant-saas' },
            { label: 'Threat model', slug: 'security/threat-model' },
            {
              label: 'Authentication',
              items: sidebarSection('security/authentication'),
            },
            {
              label: 'Authorization',
              items: sidebarSection('security/authorization'),
            },
          ],
        },
        {
          label: 'Background work',
          items: [
            { label: 'Queue', items: sidebarSection('queue') },
            { label: 'Scheduling', items: sidebarSection('schedule') },
            { label: 'Events', items: sidebarSection('events') },
          ],
        },
        {
          label: 'Operations',
          items: [
            { label: 'Testing', items: sidebarSection('testing') },
            { label: 'OpenTelemetry', items: sidebarSection('opentelemetry') },
            { label: 'Server-Timing', slug: 'server-timing' },
            { label: 'Health checks', items: sidebarSection('health') },
            { label: 'Rate limiting', items: sidebarSection('rate-limiting') },
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
