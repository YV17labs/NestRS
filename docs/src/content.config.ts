import { defineCollection, z } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';
import { TIERS } from './sidebar.mjs';

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    // `tier` places a page in its section's Basics / All options group
    // (STYLE.md §G). Declared from the same list the sidebar and the linter
    // read, so a typo is a build error rather than a page that vanishes.
    schema: docsSchema({
      extend: z.object({
        tier: z.enum(TIERS as [string, ...string[]]).optional(),
      }),
    }),
  }),
};
