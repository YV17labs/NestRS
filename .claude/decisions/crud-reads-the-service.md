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
  pair through a private trait the struct half declares beside the host,
  `__NestrsInjected<Host>`, whose associated types are named for the
  `#[inject]` fields and implemented as their types: a private trait's impl
  is no more visible than the host's module, so a host may inject a type less
  visible than itself, and a host declared in a function body has the trait
  in its block. The impl half reaches it through the path its self type is
  written with (`impl super::UsersController` →
  `<super::UsersController as super::__NestrsInjectedUsersController>::svc`),
  so a name the host does not inject fails at the key: the trait has no such
  associated type, rustc proposes the injected field's name, and the host has
  no such field. The expansion reads the entity, the inputs and the wire type
  through hidden aliases of the projections. `ops` is the one selector, all
  five by default, so an op whose trait the service lacks fails to resolve,
  and a block that left `create` or `update` out over a service implementing
  the trait serves it once migrated: the upgrading guide says so, and `ops`
  keeps it closed. The four other keys are gone, none kept as a second way.
- **`Exposed { type Wire }`, sealed, on every `#[expose]`**: the one
  association from an entity to its wire type, which `PkLoadable` and
  `RelatedTo` read too. A hand-written impl would put on the wire a type no
  `#[expose]` built.
- **The GraphQL field names and the default summaries are read off the
  host's name**, the module's noun singularized (`UsersResolver` → `users`,
  `user`, `create_user`): the same names as the output type gave for every
  resource named by the convention, and still no collision for a host named
  in the singular. The singular is the generator's naive rule, so a host
  named in the singular over a noun ending in `s` loses it (`StatusResolver`
  → `statu`); the convention names it in the plural (`StatusesResolver`).
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
- **A public trait the struct half implements on the host, naming the
  field's type as an associated type** (`InjectedField<KEY>`, keyed by a hash
  of the field's name), first built. An associated type of an impl as visible
  as the host cannot name a less visible type (E0446), so a `pub` controller
  injecting a `pub(crate)` service — what a module that does not export its
  service writes — stopped compiling, `#[crud]` or not; no chain of public
  impls reaches a less visible type from a public host. Its refusal also
  quoted two 64-bit keys per error, six times over.
- **An alias per field beside the host, or a hidden module re-exporting
  them under the fields' names.** The alias's refusal of a misnamed key had
  rustc propose another alias's mangled name as the key; the module's
  `super::` cannot reach an item declared in a function body.
- **Writing the `Location` at the edge, after every layer.** The edge would
  copy the request's URI on every request, self-mounts included; the route's
  tail holds the request already, and runs only on routes.
- **A probe in each route's wrapper turning a `Created` into its response
  with the request.** A shaper (`#[http_code]`, `#[response_header]`) or a
  `Created` turned into a `Response` by the handler would bypass it; the
  marker travels with the response whatever converts it.

The limit of the path: a block naming its host through a `use … as` rename
or a type alias, or from outside the struct's module and its descendants,
does not reach the trait and fails to resolve the field's type.

**2026-10-31 — the limit is said in the struct's words.** rustc's refusal of
such a block named the private trait (`cannot find trait
__NestrsInjectedNotes`, `trait __NestrsInjectedNotesController is private`)
and nothing a developer wrote. The struct half now writes, beside the trait,
the host's name, its module and the sentence, as hidden inherent consts the
type reaches whatever names it; `#[crud]` compares them with the name it is
written under and the module it expands in, in a `const`, and refuses a block
that cannot reach the trait: "`#[crud]` reads `NotesController`'s injected
fields through the struct's own name and module: write it on
`impl NotesController` in the struct's module, or on the path to it from a
module below (`impl super::NotesController`)". rustc's own error stays beside
it: the projection is resolved by name before any `const` runs, and reaching
the field's type another way is the path refused above (a key hash, an alias
per field). A block below the struct's module naming it through a named
import (`use super::NotesController;`) passes the check — a `const` sees no
import — and meets rustc's error alone; a glob import of the parent reaches
the trait, so refusing every bare name below the module would refuse working
code.

Not done: `#[expose(service = …, writes = [create, update, delete])]` emitting
the service's trait impls, the audit's optional second step. It would move the
service's capabilities out of its own file, which a reader of the service no
longer sees; reopened when a feature's glue breaks the bar `features.md` sets.

**2026-10-31 — an answer is documented and stamped by its type, whoever
returns it.** Under the owner's agreement to the identity audit's second pass
("Tout faire (Recommandé)", 2026-10-31), the fourth bullet above changes in
two halves:

- **The body is read by its type.** `#[routes]` read the status and the
  header through a type probe but the body by its spelling (`Json<T>` under
  one `Result` and one `Created` or `Paginated`), so an alias documented `201`
  and its `Location` with no body, a renamed `Result` dropped the body, and a
  typed answer rendering its own JSON — the token response — was restated in
  `#[api(response = T)]` at every route returning it. A second probe,
  `DocumentedPayload`, names the type the body's schema is read off. It is
  implemented for `Json<T>`, `Created<B>`, `Paginated<B>` and `Result<R, E>`,
  and is not sealed, so `nest-rs-oauth-server` implements it for
  `AccessTokenResponse`. Its schema bound sits on the probe's method, which
  the selection never reads, so a `Json<T>` whose `T` has no schema stays a
  compile error rather than a body the document drops. `#[api(response = T)]`
  stays, for a handler building its own `Response`, and wins where both
  speak: refusing it beside a probed body would be a break, keeping it is not.
- **The headers are written whoever answers.** Only the `#[routes]` tail
  wrote a `Created`'s `Location` and a `Paginated`'s `Link`, so a self-mount's
  entry or a route registered by hand answered `201` with no `Location`.
  Both tails that run a developer's code now write them through one function
  (`site::stamp_answer`); the endpoint's tail, which hands the request over
  whole, keeps the caller's URI first — a reference-counted clone per such
  request, nothing more on a `#[routes]` route. The refusal above of writing
  at the edge still holds: the edge would pay on every request, where the
  tails pay only where a typed answer can come from.

Refused: sealing the probe in `nest-rs-http`, since the token response is the
OAuth server's; `ProblemDetails` as a member, an error body documented through
`error(…)`; `(StatusCode, B)` as a member, whose status is a value, so the
document would file the body under the default status.

## 2026-10-31 — `service` names the type, reached through `Injects<S>`

Under the owner's agreement to the identity audit's second pass ("Tout faire
(Recommandé)", 2026-10-31), which covers this break of `#[crud]`'s key.

The per-host trait made every misnamed key or misplaced block lead with an
expansion name (`cannot find associated type svc in trait
__NestrsInjectedNotesController`), and one shape had rustc's help tell the
developer to import that trait into their own source; a block that worked in
one file stopped working under a `use`.

Decided, by elegance with correctness held: **`#[crud(service = NotesService)]`
names the service's type**, and `#[controller]` / `#[resolver]` implement a
hidden `Injects<S>` for each `#[inject] Arc<S>` field, which every generated
operation reaches the field through, by one hidden function spanned on the
key. The refusal of E0446 above does not apply: a trait keyed by its type
parameter puts the service in the impl's header, and rustc makes an impl as
visible as the least visible type there, so a public host injecting a
crate-private service compiles and no associated type names it. The per-host
trait, its consts, the placement check and its sentence are gone, and so is
the limit of the path: a block naming its host through an alias, an import
or another module reaches the field as the struct's module does. A host
injecting no `S` reads one sentence, "`NotesController` injects no
`NotesService`", with the field to add.

What it costs: a host injects each type once — two impls of one key overlap
(E0119), so the second field of one type is refused at the struct, which the
container's one provider per type already made redundant — and a field whose
type names a host type parameter gets no impl, since two such impls could
overlap. The check compares spellings, all an expansion reads: two fields
naming one service two ways (`UsersService` and `users::UsersService`, or an
alias), or under two `cfg`s that both hold, reach rustc's E0119 at the
struct's decorator, naming the hidden `Injects<S>` — the one refusal of the
shape not in the framework's sentence, on a host 6.x compiled. A lowercase
key, the earlier spelling, is refused naming the type to write.

Refused: **keying by the field's type** (`Injects<Arc<S>>`): the refusal would
print `std::sync::Arc<…>` in place of the service a developer names, and an
`Option<Arc<S>>` field would take a key no `#[crud]` reads.
