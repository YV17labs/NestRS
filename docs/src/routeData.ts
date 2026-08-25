import { defineRouteMiddleware } from '@astrojs/starlight/route-data';

import { DEFAULT_DESCRIPTION as defaultDescription } from './brand.mjs';

/** Per-page Twitter tags; JSON-LD on the docs home only. */
export const onRequest = defineRouteMiddleware((context) => {
  const { head, entry, id } = context.locals.starlightRoute;
  const title = entry.data.title;
  const description = entry.data.description ?? defaultDescription;

  head.push(
    { tag: 'meta', attrs: { name: 'twitter:title', content: title } },
    { tag: 'meta', attrs: { name: 'twitter:description', content: description } }
  );

  // The docs home, whose content-collection id is the empty string — `index.mdx`
  // sits at the collection root, so the loader gives it no path segment. This
  // read `id !== 'index'`, which is true of every page including this one, so
  // the block below shipped on none of them. Nothing catches that: the linter
  // walks `src/content/docs/**` only, and a `head.push` that never runs
  // renders as an absent tag rather than as an error.
  if (id !== '') return;

  // The origin is the build's, never a literal. `ASTRO_SITE` exists so a
  // preview or a fork deploy serves a different one, and a JSON-LD `@id` is an
  // identity key — a preview build claiming to *be* nestrs.dev publishes
  // structured data about a site it is not. Astro normalises `site` with a
  // trailing slash, which is the form both `url` fields want.
  const origin = context.site?.href ?? '/';

  head.push({
    tag: 'script',
    attrs: { type: 'application/ld+json' },
    content: JSON.stringify({
      '@context': 'https://schema.org',
      '@graph': [
        {
          '@type': 'WebSite',
          '@id': `${origin}#website`,
          url: origin,
          name: 'NestRS',
          description: defaultDescription,
          inLanguage: 'en',
        },
        {
          '@type': 'SoftwareApplication',
          name: 'NestRS',
          applicationCategory: 'DeveloperApplication',
          operatingSystem: 'Cross-platform',
          description: defaultDescription,
          url: origin,
          offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
        },
      ],
    }),
  });
});
