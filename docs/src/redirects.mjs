// Routes that used to exist, and where they went.
//
// `astro.config.mjs` serves them.
//
// A key is the old route, a value the live one. Both keep their trailing slash:
// that is the form Astro serves and the form a page writes in a link.
export const REDIRECTS = Object.freeze({
  '/graphql/dataloader/': '/database/dataloaders/',
  '/throttler/': '/rate-limiting/',
});
