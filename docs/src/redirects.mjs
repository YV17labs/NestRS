// Routes that used to exist, and where they went.
//
// `astro.config.mjs` serves them.
//
// A key is the old route, a value the live one. Both keep their trailing slash:
// that is the form Astro serves and the form a page writes in a link.
export const REDIRECTS = Object.freeze({
  '/fundamentals/extending/': '/extending/',
  '/graphql/dataloader/': '/database/dataloaders/',
  '/security/authorization/per-transport-bridges/': '/security/authorization/endpoint-guards/',
  '/throttler/': '/rate-limiting/',
});
