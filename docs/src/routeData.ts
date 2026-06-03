import { defineRouteMiddleware } from '@astrojs/starlight/route-data';

const defaultDescription =
  'Scalable Rust backend apps with native performance.';

/** Per-page Twitter tags; JSON-LD on the docs home only. */
export const onRequest = defineRouteMiddleware((context) => {
  const { head, entry, id } = context.locals.starlightRoute;
  const title = entry.data.title;
  const description = entry.data.description ?? defaultDescription;

  head.push(
    { tag: 'meta', attrs: { name: 'twitter:title', content: title } },
    { tag: 'meta', attrs: { name: 'twitter:description', content: description } }
  );

  if (id !== 'index') return;

  head.push({
    tag: 'script',
    attrs: { type: 'application/ld+json' },
    content: JSON.stringify({
      '@context': 'https://schema.org',
      '@graph': [
        {
          '@type': 'WebSite',
          '@id': 'https://nestrs.dev/#website',
          url: 'https://nestrs.dev/',
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
          url: 'https://nestrs.dev/',
          offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
        },
      ],
    }),
  });
});
