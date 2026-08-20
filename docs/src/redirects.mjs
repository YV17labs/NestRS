// Routes that used to exist, and where they went.
//
// One declaration, two readers: `astro.config.mjs` serves them and
// `scripts/lint-docs.mjs`'s `link` rule counts them as resolvable — the same
// shape `sidebar.mjs` uses for the tier vocabulary. Declared twice, a retired
// route would either 404 for readers or be reported as a dead link by the gate,
// and which of the two you got would depend on which copy you edited.
//
// A key is the old route, a value the live one. Both keep their trailing slash:
// that is the form Astro serves and the form a page writes in a link.
export const REDIRECTS = Object.freeze({
  '/graphql/dataloader/': '/database/dataloaders/',
  '/throttler/': '/rate-limiting/',
});
