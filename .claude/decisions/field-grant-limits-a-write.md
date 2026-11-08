# A grant's `.fields` limits what a write sets

Settled on 2026-11-05, from the field authorization design the owner asked for ("on ne
veut pas d'angle mort", then "facile, sécurisé sans qu'il se prenne la tête, les
meilleures performances"), its point 9, and proved first: under `.fields([Name])` on
`Update`, an active model setting another column reached the driver.

`.fields` on `Update` and `Create` restricted nothing written. `Repo::scoped_save` and
`Creatable` read no field set, while the mask used it to narrow the reply. Now
`Repo::update`, `Updatable::update`, `Creatable::create` and `create_from_active` read
the columns a write chooses before a hook, a pin or a default fills the row, through
`Ability::condition_for_columns`. A column no grant of the action permits answers
`FORBIDDEN` naming it and sends nothing. Otherwise the condition runs in the statement's
`WHERE`, or the create's re-check. A developer who writes no `.fields` on a write sends
the statement he always sent: `Ability::grants_every_column` answers first, so that path
reads no column.

**A grant per column, not one grant for all of them**, by correctness. The design's
sentence was `WHERE pk AND (OR grants whose fields cover them)`, and its branch's test
list says two grants with different columns conjoin; the two differ. The reply's rule is
that a row's columns are the union of the grants it meets, so the write's is that a row
takes a write when, for each column, it meets a grant permitting it:
`(OR grants permitting a) AND (OR grants permitting b) AND NOT (OR denials)`. One grant
for them all would refuse a write the reply's rule permits.

**A replacement carries back what it read**, by the owner's rule on the developer's
everyday code. `#[crud]`'s update is a `PUT` whose input carries every writable field
(`crud-update-is-put.md`), so counting every `Set` column would refuse every `PUT` under
a narrowed grant: `.fields` on `Update` and `#[crud]` could never meet. Kubernetes reads
an immutable field the same way, refusing it only when it changes. So under a narrowed
grant `Updatable::update` compares, and a column left as it stands stays `Unchanged`, out
of the statement, which also leaves what a concurrent writer put there.

**The comparison answers only for what the caller may read**, by security, against the
easier reading: a `PUT` guessing a column it may not read would hear `403` for a wrong
guess and `200` for the right one, which is a read. Such a column counts as written
whatever its value. "May read" is `Ability::can(Read, row)` then `permitted_fields`, so a
relational `Read` grant, which only SQL decides, reads nothing here. A `Read` grant with
no list covers a hidden column too, so `UpdateModel::reads_back` says what a reply shows:
false by default, every column for `#[expose]`'s input, which shows whatever it takes.
The price: a caller who may not read a column a `PUT` carries cannot use that `PUT`.

**What is no write of the caller's:** the key, which addresses the row and which a `Uuid`
input seeds, so `.fields` need not name it (`.when` holds it; a composite key a
hand-written input sets is `.when`'s alone, which the tenant pin already covers); what a
hook, a pin or a default fills, read after; a soft delete's stamp; and a null on a create,
since an input lowers a member it did not carry to one and the two cannot be told apart
there. That null is still sent, as it is for every caller, so a nullable column with a
database default takes `NULL` under an input carrying it, grant or no grant.

**`Manage` limits a write too**: it is every action, and `rules_for` reads its grants
under `Update` and `Create`, so a list on it holds, and an action's own grant with no list
widens it, as on a reply. `Read`'s list says nothing of a write.

**The refusal crosses `DbErr`**, as `DbErr::AccessDenied { permission, resource }`, sea-orm's
own variant for what a role may not do, with `resource` `table(column, column)`: every
caller of `Repo` already reads a `DbErr`, and `?` on `ServiceError` keeps working.
`DbOutcome::Forbidden` reads it. The column list is parsed back only by `error.rs`, which
wrote it.

**Refused:**

- **Dropping a withheld column in silence**, as an allow-list on the input would: the
  caller hears success for a write that did not land (`CLAUDE.md`, *No swallowed error*).
- **Reading the row first and judging in memory**: a round trip per write, a window between
  the two, and a relational rule nothing could decide.
- **A new error type on `Repo::update`**: every service signature and `?` breaks to say what
  a `DbErr` variant already says.
- **`DbErr::Custom` with a sentence**: a class matched by its wording.
- **An escape for a service writing a server-decided column.** It adds the column to the grant
  or moves it to `before_save`. Reopened when a caller needs one.

**The limits named:**

- A write's reply is unchanged: it shows that write's own grant, so `.fields([Title])` on
  `Update` answers `{ "title": … }`, and a GraphQL selection naming what the grant withholds
  refuses the field and rolls the write back. The list is both what the write sets and what
  its reply shows, so a reply narrower than the write cannot be declared, and a write with no
  list answers every exposed column. A reply reading `Read`'s columns is the `Masked<T>` cell's,
  the second pass.
- A row its grants do not permit each column on is `RecordNotUpdated`, a `404`, not told apart
  from a row outside the scope. A column no grant permits at all is what names itself.
- A hand-written service's refusal names the column as the table does, in `detail`: `Repo`
  holds no wire type to read the field's spelling off, where a `#[crud]`'s names the input's
  field. Closing it takes a registry per exposed entity, for a rename alone.
- A tool or a message answering `DbErr` itself answers opaquely, its error filed, as it does
  every `DbErr`; `ServiceError` is what classes one. Nothing fails open.
- `Repo::insert_unscoped` is ability-less by its bar, and reads no grant.

## 2026-11-06 — a write setting nothing holds its scope

Keeping a carried-back column out of the statement can leave a statement with nothing to
set, and sea-orm then sends none and reads the row back by its key alone, the scope sitting
on the `WHERE` it did not send. Proved against Postgres: `Repo::update` handed over a row
outside its scope, a tombstone, and through `Updatable::update` a row outside a narrowed grant.
The hole predates this decision on `Repo::update`; this decision added the path through a
`PUT`. `Repo::scoped_save` now reads the row itself in that case, under the same scope, which
keeps "no extra round trip": sea-orm's read was there already, and a write that sends an
`UPDATE` reads nothing.

Refused: sending the no-op anyway (`SET id = id`), which answers correctly but writes a row
version and fires a trigger for a write that changes nothing; and `Repo::update` refusing an
empty write, since a `PUT` changing nothing is a success.

## 2026-11-08 — a write's reply reads what its caller may read

The first limit above moved with the judged row (`mask-provenance.md`, 2026-11-08): a
write's reply is a wire row like any other, judged at `Read` where it is built, so it
shows what the caller may read whatever the write's list. Under `.fields([Id, Title])` on
`Update` and a `Read` grant with no list, a `PUT`'s reply answers every exposed column,
and a mutation selecting one the write's list leaves out answers it, the write landing.

Decided by security, against restoring the cut by the write's list, which the seaorm
e2e's `service.rs` pinned and the first e2e round of the judged row asked back: that cut
answered every exposed column under a write grant naming no list, those `Read`
withholds included, and the row whole to a caller holding no `Read` at all. Judged at
`Read`, a reply never holds what the caller could not read by a `GET`. Restoring the
write's list on the judged row's always-written keys would also take
`#[expose(nullable)]` on every column a write list leaves out, read-side schemas
included, or refuse every narrowed `PUT`, the developer cost the judged row's decision
refused.

**Refused:**

- **The reply cut by the write's list**, as the reply mask did: the openings above.
- **The intersection of the write's list and `Read`'s**: never wider than either, but a
  narrowed write list then withholds non-nullable columns, so the row cannot be built and
  every such `PUT` answers `403` and rolls back.

Held by `nest-rs-authz`'s `judged` suite (a route's and a mutation's reply to a caller
updating every column and reading three) and the seaorm e2e's `service.rs`.
