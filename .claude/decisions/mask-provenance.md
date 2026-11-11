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

- **What can hold a cut row is masked**: a tokio lock through `into_inner`, and through
  `WireRow` a `Connection` whose nodes hold rows in whatever `WireAnswer` walks: an `Option`,
  a `Box`, an `Arc`, a `Cow`, a `Result`, a lock, a `Json`, a list or a set, a map. A row
  refused inside a list or a map leaves it and the node stays, as a list answering a field
  does; an array has no place to drop it from, so its node leaves with its edge. A node
  selecting nothing is a scalar serializing whole, so it reads every exposed column as
  selected.
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
  upstream could make a `Connection` `Clone`. So do the objects a `Connection` and its `Edge`
  hold beside their nodes: every wrapper forwards their `create_type_info` to the probe, as it
  does the refusal above, and `EmptyFields` walks to nothing. A column a flatten or a merge
  copied from an object a `#[field_resolver]` serves it on counts as served, since it reads
  through that object at run time: the copier holds every column of an object nothing links.
  The check runs at boot and reads nothing per request; a resolver that answers no such shape
  pays nothing.

**Refused:**

- **A mask at the wire type's own resolution** (a `guard` on `SimpleObject`, or a hand-written
  `OutputType`), which would close every shape in one place: it runs per column of every row,
  cannot know the root's action (a reply under `Update` would be judged at `Read`), and would
  refuse what `unmasked` opens. That is the cell at the row, the second pass.
- **A key on `#[field_resolver]` opening the object** (`unmasked`): a public addition for a
  shape with a four-line remedy that keeps the mask.
- **A warn in place of the refusal**: a line a deployment reads later, while the column ships.

**What the mask files repeating under every parent is one line per operation**, a refused
selection, a field no ability reached, a row it cannot read back: each is one fact, and its
count must not be the caller's to choose. A refusal and a row it cannot read back name their
field (2026-11-06), so two fields each file theirs, a filter on either finding its own; a
field no ability reached names none. The record sits in the operation's own data, built
by the first line, so an operation refusing nothing pays one empty value. Refused: holding it
in `AmbientAbility`, as this entry first did, or in the `Ability`: a graphql-ws socket holds
one holder for its whole life, so its second probe of a column filed nothing, and a guard on a
field holds one per parent, so it filed per row. **A field resolver with no ability
fails closed** where the entry above let it pass: the answer can tell, since nothing decided
what the caller reads, and reads there already refuse. **A `null` in a list passes the mask**:
it holds no row; a root's map stays refused, a key being no column and a map no row on a value.

**The limit named, each pinned by a test so that closing one reads there:** a scalar the body
builds (`Json<Holder>`, a `Json<Wire>` or a map column of an object of its own,
`serde_json::Value`, a `hashbrown` map) names nothing a type or a schema can read. A flattened row beside a masked field answering the same wire type leaves a link, so
the boot reads no orphan; telling it from an object that merely shares a small wire type's
columns (`Category { id, name }` beside a field answering `Tag { id, name }`) would refuse that
everyday shape. A column renamed onto its field resolver's name
(`#[graphql(name = "star")] raw_star` beside `async fn star`) is answered by the derive first and
hidden from the registry by the resolver's field, so no registry reading can see it; left under
its own name it stops at E0592, and the refusal names `#[graphql(skip)]`. All of it closes with
the cell.

## 2026-11-06 — a field grant names a column by identity

`.fields` kept `Column::as_str()`, the table's name, while the mask compared it with the
wire's key, so `.fields([Headline])` on `column_name = "display_title"` stripped `headline`
(run, then fixed). `FieldSet::Only` stays the table's names, the identity `same_column`
already reads and the one a write's check needs; `WireModel::key_column`, emitted beside
`wire_key`, reads a key back, so every edge's `kept_key` and `Ability::mask` judge a column.
The other way, wire keys in the set, would have left a hidden column with no name to be
granted by. What the mask reads is otherwise unchanged; `Masked<T>` stays the second pass.

## 2026-11-08 — the wire row is judged where it is built

The owner's word of 2026-11-07 ("la meilleure solution, le meilleur système, beaucoup sur
la sécurité et la performance… va jusqu'au bout") closes this decision's open question:
a loaded row is judged on its whole `Model`, once, where `#[expose]`'s conversion builds
its wire row, and no edge reads a reply back. Two designs were prototyped on the real
`#[expose]`, the real routes and the real GraphQL extension: the value-less cell (D3,
`Masked<T>` on every column, as the red team corrected it) and the judged row (D4). D4
won on each value in turn.

- **The developer first, counted on the demo.** D4 changes what a grant may withhold and
  nothing a body reads: four `#[expose(nullable)]` on the users the member's grant cuts,
  the published event carrying its `Model` and the subscription building the row (three
  lines), three `unmasked` and one `#[wire_default]` dropped. The wire fields stay plain
  values, `User::from(&row)` stays the one spelling. D3 counted about 28 read sites, a
  `.get()` on every wire field a body or a test reads, `into_model()` for `Deref`, and a
  hand-written `From<Withheld>` beside each product error.
- **Security, each opening.** A withheld column is no value in the row — an `Option` column
  or one `#[expose(nullable)]` declares reads `None` — so the four shapes `TODO.md` named
  (a scalar the body builds, a scalar or map column of an object, a flatten beside a
  resolver of its type, a renamed column) and the generated loaders (decision 4 of
  `TODO.md`) hold nothing to ship. A grant on `Read` or `Manage` withholding a column no
  `null` answers is a `MalformedRuleError` at `build()` naming the column and the key, so
  OpenAPI's and MCP's `required` and GraphQL's `!` stay true (F2, F3). The row carries the
  id of the ability that judged it (minted per ability, never a pointer) and a private
  proof no struct literal supplies: its `Serialize` and each GraphQL column check that the
  ability installed where it answers is that one, so a cache, a row carried into another
  request and a publisher's push answer no one (F1), a row judged for a caller refuses on a
  push to other connections (`WsServer`), and a system row (a job's) answers no caller. A
  row its grants refuse (a write-only token's reply, F6, F7) answers its key alone on
  GraphQL and `403` elsewhere, never `{}`. A subscription's items are pushes, judged as
  `Pushed` reads them where the stream builds them, for the subscriber, and an item a
  refusal makes leaves the stream. A `belongs_to` reads its link off the judged row, so a
  withheld key reads `null` (F9's half; `has_many` keeps its `#[expose]` as its opening).
  Judged under `installed()`, a job's scope tested apart (F10).
- **Performance**, release, 100 user-shaped rows (6 exposed, 3 hidden), an ability 8
  ambient values deep, the red team's bench rerun on this host at `fb107d774` beside D4:

  | HTTP, `Vec<Model>` to bytes | none narrowed | one grant | conditional |
  |---|---|---|---|
  | the reply mask (convert, serialize, shaper) | 110.5 µs | 115.9 µs | 116.5 µs |
  | D3, the cell (a real `From`, a read per row) | 32.3 µs | 25.7 µs | 32.8 µs |
  | **D4, the judged row (real `#[expose]`)** | **31.4 µs** | **23.7 µs** | **31.7 µs** |
  | no mask at all | 24.6 µs | 24.7 µs | 24.6 µs |

  GraphQL before resolution: the mask 69.7 / 80.7 / 78.8 µs, the cell 15.3 / 16.1 /
  18.8 µs, D4 18.4 / 14.2 / 20.2 µs, plus 4.2–4.3 µs at resolution for its 600 column
  checks (the cell's stamp, 3.0–3.1 µs). D4 is the cell's cost without the cell.

**Decided with it.** `#[wire_default]`, `unmasked`, `masked_reply`, `Ability::mask` and
`mask_many`, `MaskReplyError` and every edge's reply mask and probe go: nothing reads a
reply back. A reply reads `Read`'s grants whatever the posture's action (the red team's
F6 named it; the owner's mandate decides it): `.fields` on a write grant names writable
columns, never readable ones, and a narrowed write grant could not build a row whose
columns no `null` answers. A posture keeps its gate and the reads it opens. A tool's answer
is written inside its site on MCP, rmcp reading its output schema off the authored
`Json<T>`. CASL's split of `accessibleBy` (query) from `can` and `permittedFieldsOf`
(instance) is now verified against its repository sources (`gaps/` survey, 2026-11-07).

**Refused:**

- **The cell (D3 corrected)**: everyday reads through `.get()` and `into_model()` for the
  same closure, and a cell's name or a per-cell stamp to say what D4's row says once.
- **Plain `Option` columns without a stamp** (the red team's runner-up): it loses the
  refusal of a row judged for another caller.
- **A per-row proof in the field set alone, without the ability's id**: a `with_ability`
  narrowing and an intersection would read as the caller.
- **Keeping the reply mask beside the judged row**: two judgements, the slower one on
  every reply.

**The limits named, in `TODO.md`:** a body reading a column off a whole `Model` and
answering it as its own value (`initials(row.name)`, `row.email` into a DTO) is the
body's visible line; a list a body built holding a row its grants refuse fails the whole
reply closed, where a `Repo` read never hands it one; a `#[public]` GraphQL field whose own
guard attaches the ability, its request carrying none, answers no wire row, its columns
resolving outside that field's unit; a push to other connections carries no row judged for
a caller, so a per-recipient row is built per recipient or is a DTO.

## 2026-11-08 — a row no `#[expose]` decorates is refused where a method answers it

The 2026-11-05 entry left a `Model` no `#[expose]` decorates held by no rung: deriving
`Serialize` and `JsonSchema`, with a `password_hash`, it answered `Json<secret::Model>`
under `#[routes]`. Seen again at `ac578c866`, one fixture per edge, each compiling: a
route answering `Json<secret::Model>`, `Result<Json<Vec<…>>, HttpError>`,
`Json<Option<…>>` and `#[api(response = secret::Model)]`; a message answering the row and
`Result<Vec<…>, WsError>`; a tool's `Result<Json<…>, McpError>` and a resource's
`Result<Json<Vec<…>>, McpError>`; a `#[query]` answering a row deriving `SimpleObject`, and
`async_graphql::Result<Vec<…>>`.

**Decided: a probe per dispatched method, asking the compiler of every type its answer
spells.** `#[routes]`, `#[messages]`, `#[operations]` and `#[tools]` write beside each
method an associated function, compiled and never called
(`nest_rs_codegen::__private::no_row_answered`), asking
`nest_rs_core::__private::RowProbe` of each type the return type spells — a path and
every type argument it carries, through references, slices, tuples and an `impl Trait`'s
bounds, so a stream's `Item` is asked too — and, on a route, of `#[api(response = T)]`. The
inherent arm, which method resolution prefers, exists for a `sea_orm::ModelTrait` alone,
under `nest-rs-core`'s `seaorm` feature, which `nest-rs-seaorm` turns on; its bound is a
trait no type meets, whose `on_unimplemented` says the fix at the type's own name:
``a row crosses no wire: `secret::Model` holds every column of its table``. Spelling only
finds what to ask; whether a type is a row is the compiler's answer, an alias included
when it is the leaf. An `#[expose]`d `Model` meets it beside its coherence refusal.

- **Developer first**: no line of everyday code moves — a method answers the exposed
  type or a DTO, as every page already shows — and no build without `sea-orm` holds a row
  to ask of. Compile cost: one never-called function per method.
- **Refused: `nestrs lint` on the derive.** Reads source, so a row from another crate,
  a macro's derive or a re-export escape it; a rung below the type's is the weaker one.
- **Refused: a negative bound on `Json<T>`, `WsServer::emit` and rmcp's `Json`.** No
  stable Rust writes `T: !ModelTrait`, and specialization is nightly.

**The limit named, in `TODO.md` §6:** a body serializing a row by hand —
`serde_json::to_value(&row)`, a push (`WsServer::emit(&row)`), an `#[sse]` event, a DTO
holding the row in a field, a type alias hiding the row inside a wrapper the probe cannot
open — stays that body's visible line, as the derive on the row it needs is.

**With it: a refused row's line names its unit.** A wire row refusing where it serializes
filed its line with the entity and the reason, naming the unit by its span alone (seen red
at `ac578c866`: `host` absent on HTTP's `Json`, a WS reply and an MCP tool's answer). Its
refusal is now handed to the edge answering (`nest_rs_core::__private::answering`), which
files it once at the unit its `mounted` line names — HTTP's site with the route's `host`,
`method`, `verb` and `path`, WS's reply with the message's `path` and `event`, MCP's answer
with the operation's `path` and `tool` or `resource` — and a push names its `event`; a
refusal no edge claims is filed where it is met, naming no unit, so none is lost. GraphQL's
column refusal resolves after its root field's unit, so each operation notes the site of
every root field it dispatches, by its response key, and the column names that site, once
per operation and reason: seen at `a1f890594`, the line quoted the alias a client wrote
(`"field": "chosen_by_the_client"`). A row inside a GraphQL `Json` scalar, which
async-graphql serializes itself, still names no unit (`TODO.md` §4).

**And a field resolver's parent.** A `#[field_resolver]` read its parent's columns in
Rust, past the checks each column makes where GraphQL resolves it: seen at `200bca723`,
`{ cached { emailDomain } }` answered a peer the domain of an email a publisher's row
held, judged for the admin. `#[field_resolver]` now asks
`nest_rs_graphql::__private::ParentProbe` of its parent before it runs: a wire row (core's
`WireRow`, which `#[expose]` writes beside authz's `JudgedRow`, local so a push tells a
list from a row) that refuses the caller refuses the field as its columns do, `INTERNAL`
or `FORBIDDEN`, one line per operation naming the resolver's site; any other parent
passes. Cost: one judgement check per parent and resolver, as each column pays.

## 2026-11-08 — a refused row's key answers a write's reply alone, and a job's row a job alone

The review of the judged row ran three cases the entry above names and its code did not
hold. A row its caller's grants refuse answered its key on every GraphQL operation, not on
a write's reply alone: `{ members { id } }` listed the key of a row of another org, and a
subscription item holding the row inside an `Option`, a `Result` or an object of the
body's own reached the subscriber with its `id`, only a bare row leaving the stream. And a
row a job built, every exposed column, answered whole a `#[public]` route holding no
ability (`200` with the `email` a grant withholds), the `(System, None)` arm admitting
wherever no ability was installed. Each seen red in `nest-rs-authz`'s `judged` suite and,
for a relation, `nest-rs-seaorm`'s (a `has_many` under a refused parent loaded its
children); the HTTP route's line named no unit there either, a bare site with no unit of
work filing nothing itself.

**Decided, by security, each with no line of everyday code moving:**

- **A refused row answers its key in a mutation's reply alone** (F6 asked no more): the
  column's guard reads the operation's type off the context it already holds, so a query,
  a subscription and a `has_many` under such a parent refuse the key as every other column,
  and a list a body built holding one fails closed whole.
- **A pushed item leaves the stream wherever its rows can be told**: a row, a list's, or
  either inside an `Option` or a `Result` (`PushedRows`, implemented in `nest-rs-authz`
  alone, so no downstream impl overlaps it). An item of the body's own holding a row
  arrives, its row answering no column, the key included.
- **A system row answers inside a job and on a push to other connections, nowhere else**:
  carried out of the job — a cache, a static — it is another caller's, `500` and its
  `error` line, as a row judged for another ability is.
- **The bare HTTP site files a refused reply's line** at the route, as every other site
  does.

**Refused:** dropping an object item holding a refused row after async-graphql resolved it
(a schema extension's `subscribe` hook reading each response's errors): it would read a
reply back, the design the judged row replaced, to hide only that a push arrived, which is
all such an item tells its subscriber. Kept as a limit in `TODO.md` §6; an item that must
not say even that is a row, a list, or one inside an `Option` or a `Result`.
