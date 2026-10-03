// The architecture figure on `/why/` (`src/components/Topology.astro`) draws
// this data, and `scripts/lint-docs.mjs` imports it too: `topology-drift` checks
// every app, module and edge below against what the demo's composition roots
// import, so the figure cannot drift from `demo/` unseen.

/// Every module of the demo's features crate, in the order the figure draws it.
export const FEATURES = [
  'users', 'orgs', 'posts', 'audio', 'chat', 'notifications', 'authn', 'authz', 'oauth',
];

/// Each binary of the per-workload view, as the `[module, edge]` pairs its
/// `module.rs` imports — `null` for a module imported as a port, with no edge.
export const TIERS = [
  {
    hint: 'exposed to clients',
    private: false,
    apps: [
      { app: 'auth', imports: [['oauth', 'http']] },
      {
        app: 'api',
        imports: [
          ['users', 'http'], ['users', 'graphql'], ['orgs', 'http'], ['orgs', 'graphql'],
          ['posts', 'http'], ['posts', 'graphql'], ['audio', 'http'], ['audio', 'schedule'],
          ['notifications', 'http'], ['notifications', 'events'],
          ['authn', null], ['authz', null], ['authz', 'graphql'],
        ],
      },
      {
        app: 'live',
        imports: [['users', 'ws'], ['chat', 'ws'], ['notifications', 'ws'], ['authn', null]],
      },
      { app: 'assistant', imports: [['users', 'mcp'], ['posts', 'mcp'], ['audio', 'mcp']] },
    ],
  },
  {
    hint: 'off the request path',
    private: true,
    apps: [
      {
        app: 'worker',
        imports: [['audio', 'queue'], ['notifications', 'queue'], ['notifications', 'schedule']],
      },
    ],
  },
];

/// The module the caption traces through every binary serving it.
export const TRACED = 'users';

/// The one pair of edges sharing a path, so merging every app into one binary
/// takes a rename: each file, under `demo/`, declares `path = "<path>"`.
export const COLLISION = {
  path: '/users',
  files: ['crates/features/src/users/http/controller.rs', 'crates/features/src/users/ws/gateway.rs'],
};
