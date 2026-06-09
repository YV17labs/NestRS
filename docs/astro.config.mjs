import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';
import mermaid from 'astro-mermaid';
import remarkGfm from 'remark-gfm';
import rehypeExternalLinks from 'rehype-external-links';

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
  // GFM tables/strikethrough/task-lists are not applied to .mdx by default in
  // Astro 6 — wire remark-gfm explicitly so every docs table renders.
  markdown: {
    remarkPlugins: [remarkGfm],
    // External links open in a new tab (with rel="noopener noreferrer") so a
    // reader following e.g. the SeaORM link keeps the docs open. Internal links
    // are left untouched.
    rehypePlugins: [
      [rehypeExternalLinks, { target: '_blank', rel: ['noopener', 'noreferrer'] }],
    ],
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
            'NestRS sits on top of hyper/tokio/poem. It is decorator-driven (procedural macros: #[module], #[controller], #[resolver], #[gateway], #[processor], #[cron_job], #[mcp]), with a flat type-id DI container verified at boot (the "access graph"), an ambient data context that installs a request-scoped executor and ability, row-level filtering and response masking via CASL-style abilities, and per-binary subsets through module-gated discovery.',
        }),
      ],
      logo: {
        light: './src/assets/logo.svg',
        dark: './src/assets/logo.svg',
        replacesTitle: true,
      },
      favicon: '/favicon.svg',
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/NestRS/NestRS' },
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
      },
      editLink: {
        baseUrl: 'https://github.com/NestRS/NestRS/edit/main/docs/',
      },
      lastUpdated: true,
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Introduction', slug: 'index' },
            { label: 'Why NestRS', slug: 'why' },
            { label: 'Getting started', slug: 'getting-started' },
            { label: 'CLI', slug: 'cli' },
          ],
        },
        { label: 'Fundamentals', items: [{ autogenerate: { directory: 'fundamentals' } }] },
        { label: 'Configuration', items: [{ autogenerate: { directory: 'configuration' } }] },
        { label: 'HTTP', items: [{ autogenerate: { directory: 'http' } }] },
        { label: 'Database', items: [{ autogenerate: { directory: 'database' } }] },
        {
          label: 'Security',
          items: [
            { label: 'Overview', slug: 'security' },
            { label: 'Threat model', slug: 'security/threat-model' },
            {
              label: 'Authentication',
              items: [{ autogenerate: { directory: 'security/authentication' } }],
            },
            {
              label: 'Authorization',
              items: [{ autogenerate: { directory: 'security/authorization' } }],
            },
          ],
        },
        { label: 'Testing', items: [{ autogenerate: { directory: 'testing' } }] },
        { label: 'Tutorial', items: [{ autogenerate: { directory: 'tutorial' } }] },
        { label: 'GraphQL', items: [{ autogenerate: { directory: 'graphql' } }] },
        { label: 'WebSockets', items: [{ autogenerate: { directory: 'websockets' } }] },
        { label: 'OpenAPI', items: [{ autogenerate: { directory: 'openapi' } }] },
        { label: 'Queue', items: [{ autogenerate: { directory: 'queue' } }] },
        { label: 'Schedule', items: [{ autogenerate: { directory: 'schedule' } }] },
        { label: 'Events', items: [{ autogenerate: { directory: 'events' } }] },
        { label: 'MCP', items: [{ autogenerate: { directory: 'mcp' } }] },
        {
          label: 'Observability',
          items: [
            { label: 'OpenTelemetry', items: [{ autogenerate: { directory: 'opentelemetry' } }] },
            { label: 'Server-Timing', slug: 'server-timing' },
          ],
        },
        { label: 'Health', items: [{ autogenerate: { directory: 'health' } }] },
        { label: 'Throttler', items: [{ autogenerate: { directory: 'throttler' } }] },
        { label: 'Decorators', slug: 'decorators' },
        { label: 'Glossary', slug: 'glossary' },
      ],
    }),
  ],
});
