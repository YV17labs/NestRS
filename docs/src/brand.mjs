// The sentence a page ships when it declares no `description` of its own.
//
// Read by the Astro config (the site-wide `<meta name="description">`) and by
// the route middleware (the per-page Twitter card). Two renderings of one
// claim, not two claims — so a change to it is one edit, and the two cannot
// answer differently for the same page.
export const DEFAULT_DESCRIPTION =
  'The Rust framework for modular, scalable backends — you write the business logic, it carries the rest.';
