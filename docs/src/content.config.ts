import { defineCollection, z } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';
import { TIERS } from './sidebar.mjs';

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    // `tier` places a page in one of the two lists its section's index
    // presents (STYLE.md §G) — a reading of that section, never a sidebar
    // level. Declared from the same list the linter reads, so a typo is a
    // build error rather than a page filed under a group nothing renders.
    schema: docsSchema({
      extend: z.object({
        tier: z.enum(TIERS as [string, ...string[]]).optional(),
      }),
    }),
  }),
};
