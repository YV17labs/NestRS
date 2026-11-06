# The mask decides only what the rebuilt row shows, and its provenance decides the rest

The response mask rebuilds a `Model` from the wire value: an unexposed column
takes a placeholder the derive invents, and a relation is not there at all. Each
rule then ran in memory as if the row were whole. A relation answered `true`, a
hidden column compared its placeholder, and every mask counted the answer as
decided. On a subscription item a relational grant admitted every post to every
subscriber; on a decorated reply a relational denial dropped every row its read
had admitted, and a relational grant with a wider field set widened the fields
of a row a narrower grant admitted. A mask is documented fail-closed, and
"No exposure outside `#[expose]`" is a hard "no" (`CLAUDE.md`).

**The row says what it can, as the rendered SQL would.** The in-memory check
computes SQL's truth on the row's own columns — `TRUE`, `FALSE`, or `UNKNOWN`
from a comparison with `NULL`, except that sea-orm renders `eq` and `ne`
against `NULL` as `IS NULL` and `IS NOT NULL`, which the row decides — and
*undecidable* for a relation, a column the fill may invent, an exposed column
the object left out (serde's `None` is not a value it sent), or two value types
only the database compares. Undecidable parts settle nothing a decided part does
not: `org = 7 OR related(…)` is decided on a row of org 7.

**Where the row came from decides what undecidable means.**

- **Loaded** — a decorated reply, read through `Repo` under this ability, whose
  SQL applied `(OR grants) AND NOT (OR denials)`. When no decided grant matches,
  an undecided one did; no denial did. Fields come from the decided grants, else
  the intersection of the undecided ones: whichever SQL matched, that much is
  certainly granted.
- **Pushed** — a subscription item or a `masked_reply` value, which no read
  filtered: an undecided grant grants nothing, an undecided denial denies, and
  the first refusal that undecidability alone made per entity and action files
  one `warn`, so a silent stream is visible without a line per item. OWASP ASVS
  5.0 V8: authorization fails securely.
- **Held** — a model the caller checks itself with `Ability::can`, read for
  this action or another, so no read is known to have applied this action's
  rules. A grant's relation is taken to hold, as `can` always took it, so its
  negation grants nothing; a grant undecided otherwise grants nothing, and an
  undecided denial denies. `Loaded` here would let a relational denial on
  `Update` pass a model read for `Read`.

Prior art: Postgres row-level security evaluates where the rows are; CASL keeps
`accessibleBy` (query) apart from `can` (instance) (**unverified** against
today's CASL docs).

**Refused:**

- **Deciding a pushed relational rule by re-reading the row through `Repo`
  under each subscriber's ability**: a round trip per item per subscriber on the
  hot path, inside a transaction the push path does not own.
- **Skipping an undecidable rule on a reply** (the first draft): no relational
  grant would count, so every row a relational grant admitted would drop.
- **A structural decidability check** — undecidable as soon as a relation or an
  invented column appears anywhere in the rule: it refuses a pushed row its own
  columns settle, the public-or-member shape every feed has.
- **Naming invented columns by serialized key** (`placeholdered() -> &[&str]`):
  a predicate holds `Column` values, which sea-orm gives no `PartialEq`, and a
  key is not a column name once `column_name` or `enum_name` moves one. The
  derive answers `is_placeholdered(column)` with a `matches!` over the variants.
- **Shipping a refused lone pushed object as `{}`** (every field stripped): it
  still tells the recipient a row exists. It comes back `null`, as a list drops
  the row; a decorated reply's lone row stays the one its read admitted.

**The limits kept:** a decorated reply whose rows did not come through `Repo`
under the same ability — a cache, a hand-built value — keeps the trust a read
earns, and `can` takes a grant's relation to hold on a model read for another
action. A push to other connections (`WsServer::emit`, `broadcast`) is masked
for the sender's ability, if at all, never per recipient; a per-recipient mask
on `WsServer` is deferred.

## 2026-10-14 — `can` grants nothing on a relation, and `Repo::can` decides it

The limit above is closed. `Held` took a grant's relation to hold, which is
true only of a read scoped to the same action, so `can(Update, &row)` on a row
read for `Read` granted `Update` to every user a relation-gated grant names
nowhere. A security check whose answer is wrong for its most natural call is
not safe by default.

- **`Held` now reads a rule as `Pushed` does**: an undecidable grant grants
  nothing and an undecidable denial denies; it files no warn, since its caller
  hears `false`. `can` stays synchronous and queries nothing. `Loaded` (the
  masks) is unchanged.
- **`Repo::can(action, &model)` is the instance check SQL decides**: one query
  on the model's primary key under `condition_for(action)`, on the ambient
  executor and ability, as every `Repo` read. It lives on `Repo` because it
  reads the database, which only `Repo` does; it carries `Ability::can`'s name
  because it answers the same question.

**Refused:**

- **A typed proof of a same-action read that `can` would accept.** `Repo::scoped`
  returns sea-orm's `Select`, which carries no marker into the rows it yields,
  and changing that return type would move every custom query. The proof that
  exists, `Authorized<A, E>` from `CrudService::access`, lives above
  `nest-rs-authz` and is already the answer: a row bound for `Update` needs no
  check. So `Repo::can` is the one spelling.
- **A flag or a second in-memory method restoring the old reading**: it would
  be the unsafe default under another name.

**The limit kept:** a decorated reply whose rows did not come through `Repo`
under the same ability keeps the trust a read earns, as stated above.

## 2026-10-31 — `#[expose]` writes the round trip, and a column it cannot rebuild does not compile

The rebuild above went through serde on the `Model`: every masked entity
wrote `model_attrs(derive(Serialize, Deserialize))`, its crate a `serde`
line, and a `Model` that serialized was accepted by `Json` and a WS reply with
its hidden columns, on a route no mask shaped. A hidden column of a type with
no automatic placeholder — a tenant `Uuid`, a timestamp — compiled, and failed
every masked reply carrying its row with a `500`, although the decorator saw
the field. The owner agreed to the change, a public API break, with the
identity audit's second pass ("Tout faire", 2026-10-31).

- **`#[expose]` emits the round trip on the entity, in `nest-rs-authz`'s
  `__private`**: each exposed column read and written through its own serde
  impls, each hidden one given a placeholder written as a Rust value, the
  invented columns and the wire keys named beside them, as before. One trait
  on the entity, so every mask takes one bound. The `Model` derives no serde.
- **A serde derive on the row is refused at the derive** — in `#[derive]`,
  `model_attrs` or `model_ex_attrs` — naming the exposed type a reply answers
  and the inputs a write takes: every 6.x entity carries one, and with it
  `Json(model)` would compile again, its hidden columns on the wire, and
  `Json<Model>` would read every column off a body no input validates. A serde
  impl the decorator cannot read — written by hand, or derived above
  `#[expose]` — stays the one opening, an explicit line review reads.
- **A hidden column with no placeholder is refused at the field**, naming
  `#[wire_default]`, `#[wire_default(expr)]` and `#[expose]`; a hidden primary
  key is refused naming `#[expose]`, since a key is never invented.
- **A hidden column is never read off the wire**: its placeholder stands,
  whatever an object carries under its key, which a rule never decided on
  anyway.
- **The mask cuts the wire row** to its exposed, granted keys in place rather
  than writing the rebuilt model again, and `Ability::mask` writes the exposed
  columns alone — before, an unrestricted grant handed a hidden column out of
  it.

The provenance reading above is unchanged.

**Refused:**

- **A `Serialize` on the `Model` emitted by `#[expose]`** (routed through the
  framework, so no manifest line): it keeps `Json(model)` compiling for every
  entity, the path this closes.
- **A type-level check that the `Model` implements no serde**, which would
  also catch a hand-written impl: an impl-ambiguity assertion reports
  `type annotations needed` naming the expansion's trait, and a `Serialize`
  impl bounded on a trait never implemented reports serde's own missing-impl
  error, advising the derive — neither is the one sentence a misuse reads.
- **Skipping the rebuild when no rule of the caller's has a `when`**, keeping
  only the cut to exposed keys. Measured in release on a 100-row list of a
  user-shaped entity: the rebuild costs 23.5 µs of the shaper's 100.5 µs
  (parse and serialize alone 51.4 µs). The rebuild is also the check that the
  body is the entity's: without it, `Json<Vec<Post>>` under
  `#[authorize(Read, User)]` would ship its matching keys to an unconditional
  caller instead of failing closed, so the fail-closed check would depend on
  who calls.
- **`Default` for every hidden column**: a type without one would fail inside
  the expansion naming the trait, not the column, and an alias of a scalar
  could not be told from a domain type; the narrow set and an explicit
  `#[wire_default]` keep the sentence at the field.

**Still open, the owner's:** judging a loaded row on its `Model` before the
wire conversion, so a reply's rows are never rebuilt (this decision's
`Loaded` reading for loaded rows); and serving a field a grant may mask as
nullable on the GraphQL wire type rather than requiring an `Option` column.

## 2026-11-05 — a relation's field masks by its selection, and its batch filters rows

A relation's batch masked each row into the wire type with no `Context`, so a
`.fields` grant narrower than the type's non-null columns failed the deserialization
for every row: a member's every read of a post's author answered `INTERNAL` and filed
an `ERROR`, whatever it selected, where the same selection answers on a root. It failed
closed, and it leaked nothing. A second defect stood beside it, by the same absence: a
`#[field_resolver]` answering a wire type ran no mask, so a field grant did not hold
there at all.

- **The mask runs where a selection exists, the field, sharing a root's rule**
  (`refused_selection`, `mask_row`). The batch reads through `Repo` and builds each
  row's wire type, so what a batch holds depends on no selection, and each alias
  answers by its own. Correctness, then security: the row stays judged `Loaded`,
  rebuilt from the wire as a root's is; judging it on its `Model` stays this decision's
  open question, the owner's.
- **A caller whose grants narrow no column skips the round trip on a relation.** The
  rebuild stays on every row of a reply because it is also the check that the body is
  the entity's, and a relation's row is the entity's own wire type, built from the row
  the batch read through `Repo`: the type holds that check, and SQL has judged the row.
  So the promise holds that the hot path pays for nothing it does not use. Measured in
  the commit that made it, against the 0.23 µs per row above.
- **A hand-written `#[field_resolver]` is masked with no fast path.** Its rows did not
  provably come through `Repo`, so a list's refused rows drop as a root's do; security
  outranks the round trip per answer. The entity is read off the answer's type, through
  a hidden impl `#[expose(…, graphql)]` emits on the wire type and a probe at the
  expansion walking down what async-graphql serves around it, so no key is added, what
  holds the rows owes the mask no serde impl, and an answer holding no wire type pays
  nothing.
- **A body's answer takes its provenance off its caller.** A caller a token backs reads
  through `Repo` under this ability, so its answer is a reply, keeping the trust a read
  earns whatever read it, the limit above. A visitor's reads are withheld below its root,
  so no read can have applied its rules, and its answer is a push: judging it `Loaded`
  would trust a read that cannot have run.
- **A field with no selection refuses every key the mask took.** `Json<Wire>` is a scalar:
  it serializes whole, so "a key outside the selection is never serialized" did not hold
  of it, at a root as at a nested field, and it shipped what the grant withheld. A list
  relation's page keeps its empty selection, `pageInfo { hasNextPage }` reading no row.

**Refused:**

- **A sealed row type as a loader's value**, so a body could not read a column out of
  it: it breaks every resolver reading `<Service>ById`, the documented path, and a body
  holds the same columns from `Repo` anyway. A scalar a body derives from a row is its
  own line, as it always was off `Repo`, though the batch's mask used to refuse it off a
  loader: an exception to "whichever path reads it", put to the owner.
- **Serde's `rc` feature and a serde bound on the wrappers**, reading an answer whole: a
  `Connection` and an array past 32 rows have no serde impl, so each would have stayed
  outside the mask with nothing said.
- **A second, hidden key on each generated loader**, keeping the public one masked at
  its batch: two paths for one read, and the public one still answering `INTERNAL` to a
  legitimate read.
- **A key naming the entity on `#[field_resolver]`** for an answer whose type names
  none: a public addition, the owner's.
- **Opening a visitor's nested read of an entity.** It takes more than this mask: a
  licence per root field, `VisitorReadNamed` being one unit's and a request carrying
  several roots, and a line saying what it opens, since `define_visitor`'s grant opens
  the operations naming its entity and a relation's target is named by none. It stays
  closed, and fails so.

**The limit named:** a field grant cuts a row's columns, never the link a relation
follows, so `posts { author { id } }` answers though `.fields` withholds `author_id`,
and `users { posts { … } }` lists what a child's grant withholds the key of. A root has
always done so. Closing the `belongs_to` half costs the mask a table of each relation's
key; closing the `has_many` half refuses `orgs { users { name } }` to a grant of
`[Id, Name]`, which reads as a granted selection, so it is the owner's.

## 2026-11-05 — a row crosses no wire, by coherence

The 2026-10-31 entry left one opening to review: a serde impl `#[expose]` cannot read,
written by hand or derived above it. The owner's mandate of 2026-11-05 ("on ne veut pas
d'angle mort") reopened it, public breaks agreed. Run first, at the tip: each of these
compiled, and `#[query] fn row(&self) -> post::Model` answered through a `SimpleObject` on
the row, so a GraphQL object was the same opening, on no list.

- **`#[expose]` emits on the row the impl of every trait that carries it**: serde's
  `Serialize` and `Deserialize`, `JsonSchema`, which publishes each hidden column's name
  and type, and, under `nest-rs-seaorm`'s `graphql` feature, `OutputType` and `InputType`; on the
  `Model`, and on the `ModelEx` `#[sea_orm::model]` writes beside it, which holds every
  column too. Each is bounded on `for<'a> HiddenColumns<'a>: AModelCrossesNoWire<fn(Model)
  -> Wire>`, which has no impl and can have none.
- **What that does, read through the real decorator.** A use, whatever holds the row
  (`to_string(&model)`, `Vec<Model>`, a DTO's field, `Json<Model>`, a resolver's answer),
  is `E0277` with the bound's own sentence, naming the exposed type. Another impl of
  those traits is `E0119`: by hand, a derive above `#[expose]`, `#[output]` or `#[input]`
  on either side of it, `#[Object]`. Answering the bound is `E0117`. The derives the
  decorator can read keep their sentence, and it then emits none of these, so one
  misuse is not said twice.
- **The bound's shape is what makes it hold**, each part compiled. Everything it names is
  foreign to the crate writing it: a trait local there reports an impl beside the emitted
  one as `E0283`, `type annotations needed`, instead of `E0119`, so it lives in
  `nest-rs-seaorm`'s `__private`. The lifetime keeps it from reading as trivially false,
  `E0277` at the emitted impl itself. `fn(Model) -> Wire` names the exposed type in rustc's
  sentence and stays foreign: the bare type as the parameter reports `E0283` too, and lets
  the crate answer the bound.
- **Nothing is called.** The bodies call the bound's `refused() -> !`, so an expansion
  carries no panic path, and the mask's round trip needs no serde on a row.
- **`macros.md`'s "naming no expansion item" takes an exception here**: `E0119`'s first
  line is rustc's and its note names the bound, whose names read as the reason. Security
  over diagnostics: nothing else refuses an impl that must not exist.

**Refused:**

- **The const probe** (`const _: () = assert!(!SerdeProbe::<Model>::IMPLEMENTS, "…")`).
  Compiled through the real `#[expose]`, it has the better line for a hand-written impl,
  `E0080` with the framework's sentence alone. But `to_string(&model)`, the misuse a
  developer actually writes, stays serde's `E0277`, advising the derive `#[expose]`
  refuses and listing 468 impls, and `impl Deserialize<'static> for Model` passes its
  `for<'de>` bound in silence, and reads a row off a `&'static str`.
- **The refusals of 2026-10-31 stand corrected**: its "`Serialize` impl bounded on a trait
  never implemented reports serde's own missing-impl error" held for the form tried then.
  A foreign trait under a higher-ranked bound reports the trait's own sentence.

**A blanket of the app's own moves.** `impl<T: Serialize> Audited for T` beside `impl Audited
for Model` compiled, and is now `E0119`: coherence reads the bounded impl as one upstream could
make hold. The specific impl cannot move to the exposed type, which meets the blanket and
overlaps it too (compiled). The row's impl goes, the blanket answering for the exposed type,
or the blanket takes a bound of the app's own. Named as a break in the guide's table.

**The limit named:**

- A struct a body fills from a row's columns, `json!({ "hash": row.hash })`, and a
  `#[serde(remote)]` mirror are the body's own lines, which no type tells from
  `initials(row.name)`. `Debug` of a row in a log stays review.
- A `ModelEx` is read off the attribute's name, so a `#[sea_orm::model]` renamed on import
  leaves its `ModelEx` out: closing it takes sea-orm naming the type where a decorator can
  read it.
- **GraphQL holds with the `graphql` feature, no other way.** With `seaorm` alone and
  `async-graphql` declared by the app, `#[derive(SimpleObject)]` above `#[expose]` or
  `#[Object]` on the row compiles and answers a hidden column from a field (compiled and
  built into a `Schema`; the same derive with the feature on is `E0119`). Only the derive the
  decorator reads stays refused there. Closing it takes linking async-graphql into every
  `seaorm` build, which the third value refuses: a build with no GraphQL pays for nothing.
- **A row with no `#[expose]` is held by no rung.** A `Model` deriving `Serialize` and
  `JsonSchema`, with a `password_hash`, answers `Json<secret::Model>` under `#[routes]`
  (compiled): the project's hard "no" on exposure outside `#[expose]` is review there.
  Closing it takes a probe per edge refusing a payload that is a `ModelTrait`, a branch of
  its own.

## 2026-11-06 — a shape the mask cannot read refuses, and an object holding a row is one

The owner's mandate of 2026-11-05 ("on ne veut pas d'angle mort"), narrowed the same day: easy
for the developer, secure with nothing to think about, best performance, and no work on a bug
before a run shows it. Run first, under a grant withholding `email`: a `#[field_resolver]`
answering an object holding the row, a `#[graphql(flatten)]`, a `MergedObject`, a union, an
interface, an `Edge`, `&'static Wire`, `&'static [Wire]`, `Weak<Wire>`, `Arc<Connection>`, a
tokio lock and a `Connection` of `Option`, `Box`, `Arc` or `Json` rows each answered
`"email":"withheld@example.test"`. The entry above called an object holding a wire type the
body's own line. It is not: it names the entity, and reads as no line a reviewer finds.

**Decided, by security, then elegance:** the walk masks what it can reach, and what it cannot
refuses at the first rung that can say so.

- **What can hold a cut row is masked**: a tokio lock through `into_inner`, a `Connection` of
  `Option`, `Box`, `Arc` or `Json` rows through `WireRow`. A node selecting nothing is a
  scalar serializing whole, so a `Json` node reads every exposed column as selected.
- **What has no place for one does not compile**: `&'static A`, `&'static [A]`, `Weak<A>`,
  by `WireAnswer::READABLE`, which every wrapper forwards and the field's registration
  evaluates in a `static`'s initializer. Refused: `const { panic!() }` inside `masked`, as
  the gap asked: it fires only once monomorphized, so `cargo check` and trybuild, which runs
  `check`, stay silent.
- **What only the schema can say refuses the boot**: the fallback probe hands the boot the
  answer's `create_type_info`, and a walk over that registry follows every column but one a
  `#[field_resolver]` serves, down to a type `#[expose(…, graphql)]` declared. A flattened and
  a merged object copy the columns and leave no link, so a wire type nothing links is one of
  those. `Arc<Connection>` lands here: an impl of its own overlaps `Arc<A: Clone>`, since
  upstream could make a `Connection` `Clone`. The check runs at boot and reads nothing per
  request; a resolver that answers no such shape pays nothing.

**Refused:**

- **A mask at the wire type's own resolution** (a `guard` on `SimpleObject`, or a hand-written
  `OutputType`), which would close every shape in one place: it runs per column of every row,
  cannot know the root's action (a reply under `Update` would be judged at `Read`), and would
  refuse what `unmasked` opens. That is the cell at the row, the second pass.
- **A key on `#[field_resolver]` opening the object** (`unmasked`): a public addition for a
  shape with a four-line remedy that keeps the mask.
- **A warn in place of the refusal**: a line a deployment reads later, while the column ships.

**A refused selection files one line per request**, held in `AmbientAbility`, which one request
holds, where an `Ability` may serve every request of a role; the set is built by the first
refusal, so a request refusing nothing pays nothing. A field's own guard attaching the ability
per parent files per parent: it holds a holder per row. **A field resolver with no ability
fails closed** where the entry above let it pass: the answer can tell, since nothing decided
what the caller reads, and reads there already refuse. **A `null` in a list passes the mask**:
it holds no row; a root's map stays refused, a key being no column and a map no row on a value.

**The limit named:** a scalar the body builds (`Json<Holder>`, `serde_json::Value`, a
`hashbrown` map) names nothing a type or a schema can read; a `Connection`'s or an `Edge`'s
additional-fields object is the body's, the walk reading its nodes; a flattened row beside a
masked field answering the same wire type leaves a link, so the boot reads no orphan. All of it
closes with the cell.
