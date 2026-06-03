import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';

export default defineConfig({
  site: 'https://nestrs.dev',
  integrations: [
    starlight({
      title: 'NestRS',
      description:
        'A declarative Rust framework for service backends — native throughput, an order of magnitude less RAM, types that hold end to end.',
      plugins: [
        starlightLlmsTxt({
          projectName: 'NestRS',
          description:
            'A declarative Rust framework for service backends — decorator-driven, secure and transactional by composition, multi-transport (HTTP, GraphQL, WebSockets, queues, schedule, MCP) from one feature definition.',
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
            { label: 'Tutorial', items: [{ autogenerate: { directory: 'tutorial' } }] },
          ],
        },
        { label: 'Core concepts', items: [{ autogenerate: { directory: 'concepts' } }] },
        { label: 'HTTP', items: [{ autogenerate: { directory: 'http' } }] },
        { label: 'GraphQL', items: [{ autogenerate: { directory: 'graphql' } }] },
        { label: 'WebSockets', items: [{ autogenerate: { directory: 'websockets' } }] },
        { label: 'Data', items: [{ autogenerate: { directory: 'data' } }] },
        { label: 'Security', items: [{ autogenerate: { directory: 'security' } }] },
        { label: 'Schedule', items: [{ autogenerate: { directory: 'schedule' } }] },
        { label: 'Queue', items: [{ autogenerate: { directory: 'queue' } }] },
        { label: 'MCP', items: [{ autogenerate: { directory: 'mcp' } }] },
        { label: 'Observability', items: [{ autogenerate: { directory: 'observability' } }] },
        { label: 'Configuration', items: [{ autogenerate: { directory: 'configuration' } }] },
        { label: 'Testing', items: [{ autogenerate: { directory: 'testing' } }] },
      ],
    }),
  ],
});
