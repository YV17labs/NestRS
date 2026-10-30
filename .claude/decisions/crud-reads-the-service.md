# `#[crud]` reads its types off the service, and a create answers `Created<T>`

Settled on 2026-10-30, under the owner's agreement to the identity audit's
public-API breaks ("Tout faire", 2026-10-28), before 7.0's first publish, since
a decorator's keys are API (`semver-7x.md`).

`#[crud]` took five keys — `service`, `entity`, `output`, `create`, `update` —
and each edge restated all of them, though the service's `CrudService`,
`Creatable` and `Updatable` impls and the entity's `#[expose]` already declared
every one: a resource stated its five facts three times, and a mismatched
`output =` compiled when it had a `From<&Model>`. A hand-written create erased
its return to `Response` to stamp a `Location`, took a `&Request` only for its
path, and restated its payload in `#[api(response = T)]`; its document declared
no `Location`.

Decided, by elegance with correctness held:

- **`#[crud(service = svc)]` alone.** The field's type crosses the decorator
  pair through `InjectedField<KEY>`, which `#[controller]` and `#[resolver]`
  implement per `#[inject]` field, keyed by a hash of the field's name both
  halves compute alike; the expansion reads the entity, the inputs and the
  wire type through hidden aliases of the projections. `ops` is the one
  selector, all five by default, so an op whose trait the service lacks fails
  to resolve. The four other keys are gone, none kept as a second way.
- **`Exposed { type Wire }`, sealed, on every `#[expose]`**: the one
  association from an entity to its wire type, which `PkLoadable` reads too.
  A hand-written impl would put on the wire a type no `#[expose]` built.
- **The GraphQL field names and the default summaries are read off the
  host's name**, the module's noun singularized (`UsersResolver` → `users`,
  `user`, `create_user`): the same names as the output type gave for every
  resource named by the convention, and still no collision for a host named
  in the singular.
- **`Created<T>` and `Paginated<T>` answer through `IntoResponse` and mark the
  response; the route's tail writes the `Location` or the `Link`** on the path
  the caller addressed, as `set_created_location` and `set_next_link` built
  them (`pagination-link-header.md` holds). `#[routes]` documents the status
  and the header off the return type through a type probe, an alias included.
  The free functions left tier 1.

**Refused:**

- **`service = svc` as a default.** The key says which field every operation
  delegates to; a convention the reader must know is a second spelling of
  the same block.
- **A `name = "…"` key for the GraphQL fields.** It would restate the noun the
  host's name already carries.
- **An alias the struct half declares at module level, named after the
  host.** It resolves only where the struct is in scope; the trait on the
  host resolves wherever the impl is.
- **Writing the `Location` at the edge, after every layer.** The edge would
  copy the request's URI on every request, self-mounts included; the route's
  tail holds the request already, and runs only on routes.
- **A probe in each route's wrapper turning a `Created` into its response
  with the request.** A shaper (`#[http_code]`, `#[response_header]`) or a
  `Created` turned into a `Response` by the handler would bypass it; the
  marker travels with the response whatever converts it.

Not done: `#[expose(service = …, writes = [create, update, delete])]` emitting
the service's trait impls, the audit's optional second step. It would move the
service's capabilities out of its own file, which a reader of the service no
longer sees; reopened when a feature's glue breaks the bar `features.md` sets.
