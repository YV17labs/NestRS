# Repo hands out no connection: its selects run on the ambient executor

A service reaches the database through `Repo` alone, and `Repo` hands it no
connection. `Repo::scoped`, `with_tombstones`, `unscoped` and `unscoped_by_id`
return a `RepoSelect<E>`, which implements sea-orm's own `QueryFilter`,
`QueryOrder`, `QuerySelect` and `QueryTrait` and runs on the ambient executor
through `all`, `one`, `count` and `exists`; `insert_unscoped` and
`resolve_unique_slug` read the ambient executor as `update` does. The handle a
unit installs is a crate-private wrapper (`Installed`) whose `as_any` hands out
itself, so a downcast of `nest_rs_database::current_executor()` finds no type a
service can name. Decided by the owner on 2026-10-28 ("Tout faire", the
identity audit's api-1, feat-15 and api-9), security first.

Until then `Repo::conn()` returned the raw `Executor`, which implements
`ConnectionTrait`: every scoped read made a service hold it, and the same
handle ran `Entity::find().all(&conn)` or `active.insert(&conn)` with no
ability filter, through no escape the data-layer rule named. The docs taught
an ability-less read through it. Taking `conn()` away left a second route: the
port's public reader, downcast to the public enum, was the same handle.

The seaorm enum is `SeaOrmExecutor` — a driver carries its own name beside the
port's `Executor` (`architecture.md`) — and `nest-rs-database` keeps the one
install and read API: seaorm's `with_*_executor`, `current_executor`,
`ExecutorScope` and `current_executor_scope` duplicates went, a test installs
`SeaOrmExecutor::Pool(pool).into()` through the port's `with_request_executor`
or `with_job_executor`, and the port's scope-keeping `with_executor` moved to
its `__private`, the in-unit swap the driver's carry path calls.

**Refused:**

- **`Ambient`, an object implementing `ConnectionTrait` that resolves the
  ambient executor per statement** (feat-15's proposal). `Users::find()
  .all(&Ambient)` keeps the bypass under a shorter name.
- **`Repo::raw_conn()`, a named escape for the raw executor.** No caller in the
  tree needs one once the select runs itself; `CrudService`, the keyset page
  and the relation loader live in the crate and read `Repo::conn()`, now
  `pub(crate)`. A raw statement inside a unit is not offered; one that a
  product needs is a named escape with its bar, added then.
- **A typed `SeaOrmExecutor::current()` reader in tier 1.** It reopens the
  handle `conn()` closed. The reader is `__private`, for the crate's own
  suites.
- **Leaving the downcast route open and naming it in the rule.** A public
  reader plus a public handle is the bypass by another name, and a type can
  hold the line instead of review.
- **Two wrapper types, `ScopedSelect` and `UnscopedSelect`.** The escape is
  named at the call (`Repo::unscoped`), which is what a reviewer searches; the
  value it returns runs the same way.
- **The ability's condition applied when the select runs, not when it is
  built.** `build()` would then render SQL the database never receives.
- **`paginate` on the select.** The framework's one pagination is the keyset
  `Repo::page` (`/database/pagination/`); an offset page is `limit`,
  `offset` and `count` on the same select, and sea-orm's `Paginator` borrows a
  connection the select no longer hands out.
- **Projections and joined fetches (`into_tuple`, `find_also_related`) on the
  select.** A joined related entity would come back under the main entity's
  filter alone; a related row comes through its own service's loader, under
  its own. A projection is added with a caller that needs it.

**Limits — what the types leave to review:**

- **A statement rewritten through `query()`.** Each of sea-orm's builder
  traits requires a `query()` handing out the `SelectStatement` as `&mut`, and
  a trait's method cannot be sealed: replaced (`*query() = E::find()
  .into_query()`), cleared (`take()`) or `union`ed with an unfiltered
  `SELECT`, the statement runs past the filter through no named escape. It is
  the one way past the filter through `Repo`; dropping `QueryTrait` alone
  would not close it, since `QueryFilter::query` is the same handle. A
  `.query()` on a `Repo` select is a review trigger, grepped like an escape's
  name.
- **The ambient installers.** `nest_rs::authz::with_ability` runs `Repo`
  under the ability it is handed, and `nest_rs::database::with_job_executor`
  grants the job scope to its handle outside every unit — a test, a task
  spawned off one. Inside a unit the installer never widens it: a request,
  or a body continuing one, stays a request, held by the port's tests, since
  a re-install of the unit's own handle under the job scope turned a read
  that failed closed into an unscoped one. Both are named in the data-layer
  rule's escapes; closing them is taking them out of the public API once a
  service test opens its unit under an ability through the testing harness.

Implementing sea-orm's builder traits is not the wrapper
`public-dependencies.md` refuses: the builder methods are sea-orm's own, with
their documentation; only where the statement runs is the framework's.

## 2026-10-31 — a service test runs in a unit of the harness

`TestApp::unit(ability, || svc.list())` and `system_unit(|| …)` (and
`HeadlessApp`'s) run a body in one unit the app's bound `UnitOfWork` opens,
under the ability given or as system work, committed on `Ok`: the demo's
service tests stopped installing `SeaOrmExecutor::Pool(..)` and an ability by
hand. The ambient installers' residual named above loses its last test
caller in the demo; taking them out of the public API is the next branch's.

## 2026-10-31 — no public function widens what a unit runs under

The ambient installers' limit above is closed, with the owner's agreement to
break the API before 7.0 publishes ("Tout faire", 2026-10-28), security
first. `nest_rs::authz::with_ability` only narrows: inside a unit holding an
ability it installs the intersection — rules of both, a subject allowed only
where both allow it, conditions conjoined, columns both expose, either
denial denying, either's anonymity anonymous — in a job, which holds none, the
ability given, and elsewhere nothing, so a read that failed closed still
does. The ability is ambient through `AmbientAbility`, a type only
`nest-rs-authz` builds (its constructor is `__private`, for the testing
harness and the framework's suites), so `with_ambient` and a layer's
`cx.attach` install none: `Ability` no longer implements `AmbientValue`, and
the ability guard is the one place an ability comes from.
`with_request_executor` and `with_job_executor` moved to the port's
`__private`; the testing harness's `TestApp::unit` and `system_unit` open a
service test's unit, and `GraphqlSocketBuilder::ability` subscribes as a
caller.

The two public ways left in are held by behaviour. `run_unit`, the edge port,
tags no scope wider than the unit it opens in, as the installers do: a job
unit opened inside a request runs under the request's. A capture re-entered
inside another unit (`RequestContinuation::scope` and `enter`,
`TaskContext::carry`) holds each value as its `AmbientValue::reenter` returns
there (`ambient-carry.md`, 2026-10-31): the executor's scope no wider than the
running unit's, the ability what `with_ability` would install there, read off
`current_ability`, so a route no mask shapes stays closed. An ability
intersection keeps every condition of both sides, an empty one included:
sea-query renders an empty `any` as FALSE and a negated empty `all` as NOT
TRUE, so a part skipped for being empty read a grant admitting no row as
admitting every row.

**Limit:** outside every unit `run_unit` tags the scope its spec declares —
that is how a queue attempt, a tick, a lifecycle hook and a third-party
consumer open theirs — so feature code holding the binding
(`current_unit_of_work`, an injected `dyn UnitOfWork`) can open a job unit on
a task of its own and read unscoped. Types cannot tell an edge's task from a
feature's; the data-layer rule names it as an escape, read by grepping
`ExecutorScope::Job` in feature code.

**Refused:**

- **`with_ability` moved whole to `__private`, the job given an API of its
  own.** A narrowing cannot widen, so it needs no second name; a job acting
  for a principal is its one service use, and a second function for it would
  be a second way to install an ability.
- **Narrowing `with_ability` and leaving `Ability: AmbientValue`.**
  `with_ambient(Arc::new(ability), fut)` was the same install under the
  kernel's name, and `cx.attach(ability)` in an interceptor an authz decision
  outside a guard.
- **An empty ability installed in a request holding none.** The request's
  reads already fail closed with a line naming the missing guard; an empty
  ability would turn that wiring defect into silent empty answers.
- **The intersection kept as a pair of abilities, each read in turn.** Every
  read of an ability would branch on it; the flat rule set it builds is read
  as any other, and the cost is paid once, by the narrowing.
- **The continuation named as an escape, held by review.** A third-party
  transport and driver re-enter captures, so the functions stay public; each
  value deciding what it is inside another unit holds the rule where review
  would only read it.
- **`RequestContinuation` and `TaskContext` moved to `__private`.** The
  transport and driver pages hand them to third parties, which tier 2 does
  not serve.

## 2026-11-05 — the enum leaves with the handle

A `SeaOrmExecutor` could be named by anyone and obtained by no one, so a tier-1
type had nothing to serve. It moved to `__private` beside its reader, and
`Txn`, which only two framework tests built, is gone
(`visibility-tiers.md`, 2026-11-05). What the tests above still spell,
`SeaOrmExecutor::Pool(pool).into()`, is the suite's plumbing through both
`__private`s.

## 2026-11-05 — a `Repo` select hands out no statement

The first limit above is closed, with the owner's agreement to break the API
before 7.0 publishes ("on ne veut pas d'angle mort", 2026-11-05), security first.
`RepoSelect<E>` implements none of sea-orm's traits. Its `filter`, `order_by`,
`order_by_asc`, `order_by_desc`, `order_by_with_nulls`, `limit`, `offset`,
`apply_if` and `build` are inherent, in sea-orm's vocabulary (`IntoCondition`,
`Order`, `NullOrdering`), each one line over the `Select<E>` it holds, so
`.query()`, `.into_query()`, `.select_only()`, `.column_as()` and the rest answer
`E0599`. This reverses "implementing sea-orm's builder traits" above: a trait's
method cannot be sealed, and every builder trait's `query()` is the statement as
`&mut`. It closes what `query()` named and what the same traits left beside it:
`select_only` with `expr_as` rendered `SELECT "users"."id", "password_hash" AS
"email" FROM "users" WHERE …`, a hidden column in an exposed slot, with no
`.query()` to grep; `having` and `join` came along.

**An order key is `E::Column`**, narrower than sea-orm's `IntoSimpleExpr`. An
expression there is a second way in for a raw fragment, and an order computed over
a hidden column reads it. No caller orders by an expression, and widening later is
additive where narrowing would be a break.

**A raw fragment stays clippy's, decided on evidence.** `filter` takes
`IntoCondition`, and `Expr::cust` is one `Expr` to sea-query, so no type refuses
it. Rendered on sea-query 1.0.2, pinned by `select.rs`'s unit test:

```text
.filter(Expr::cust("1 = 1 OR 1 = 1"))     (1 = 0) AND (1 = 1 OR 1 = 1)      contained
.filter(Expr::cust("1 = 1) OR (1 = 1"))   (1 = 0) AND (1 = 1) OR (1 = 1)    every row
```

What each harder refusal would do:

- **Parentheses at the select.** One more pair holds that fragment, `(1 = 0) AND
  ((1 = 1) OR (1 = 1))`, and `1 = 1)) OR ((1 = 1` renders `(1 = 0) AND ((1 = 1)) OR
  ((1 = 1))`, every row again. A fixed depth moves the text a fragment needs, and
  the class is a value interpolated by an attacker who can count.
- **The ability's filter in a derived table** (`SELECT … FROM (SELECT … WHERE scope)
  AS t WHERE raw`). It holds an `OR`, but `1 = 1) UNION SELECT …` runs past it, so
  the class stays while every statement changes shape.
- **A walk refusing `Expr::Custom` at run.** `From<Condition> for Expr` gives a tree
  and `FunctionCall` has getters, but `CaseStatement` and a sub-select keep their
  expressions `pub(crate)`, so the walk stops short at them and a partial refusal
  reads as a whole one. It would also take the only spelling of what the typed
  builders do not reach, with no named way back.
- **Scanning the rendered text.** Unbalanced parentheses need strings, `E''`, dollar
  quoting and comments read as Postgres reads them, a SQL lexer of our own on every
  `filter`, and a miss is silent. Standards over invention.

So it is the SQL-injection class and is held where that class is, at review level:
clippy's `disallowed-methods` and `disallowed-types` in the root `clippy.toml` and a
new scaffold one. They name every call through which sea-query 1.0.2 writes text
unescaped into a query statement, read off its writers (`backend/query_builder.rs`,
`backend/postgres/query.rs`, `extension/postgres/func/`) and off what each renders
(`select.rs`'s `every_constructor_the_lint_names_splices_its_text`, one `#[expect]`
per entry, so one leaving the list fails it):

- raw text: `Expr::cust`, `cust_with_values`, `cust_with_expr`, `cust_with_exprs`,
  `custom_keyword`, `Expr::Custom`, `Expr::CustomWithExpr`, `Keyword::Custom`,
  `Func::cust`, `Func::cast_as`, `ExprTrait::cast_as`, `BinOper::Custom`,
  `OnConflict::constraint`, and `order_by_customs`, `order_by_customs_with_nulls`,
  `partition_by_custom` and `partition_by_customs` on every statement holding them
  (`From<SubQueryStatement> for Expr` reaches an `UpdateStatement` and a
  `DeleteStatement`, so those two are listed though Postgres refuses the text there);
- a JSON path, an alias or a column name, written between quotes unescaped:
  `PgFunc::json_exists`, `json_value`, `json_query`, `json_table`;
- a type name, written between double quotes unescaped: `ExprTrait::as_enum`,
  `Expr::AsEnum`, and a `sea_query::Enum`, `OptionEnum::None` or `ArrayType::Enum`,
  whose name goes out bound too, as `$1::"<type_name>"`;
- a value written inline, the other way out for that name: `Expr::Constant`,
  `Order::Field`, the second being what `RepoSelect::order_by` takes.

Executed on Postgres 18.6 under a query the ability denied whole, each of `as_enum`,
`json_table`, an `Expr::Constant` and a bound `sea_query::Enum` returned every row.
Clippy resolves a variant's constructor as a function.
`#[expect(clippy::disallowed_methods, reason = "…")]` is the opening, one line a grep
finds; the framework's own are `1 = 0` and `ROW_NUMBER()`.

**A scaffold's `crates/migrations` holds an empty `clippy.toml`**, which replaces the
workspace's there, clippy reading the nearest file upwards. A migration is DDL an
operator runs, no request's value reaches it, and `uuidv7()` has no typed builder, so
the lint would only make an everyday migration harder and refuse nothing: the owner's
rule of 2026-11-05 that a fix does not make everyday code harder. The generated
`Expr::cust("uuidv7()")` stays as it was, with no `#[expect]`.

**Limits:**

- **It holds only where an app runs clippy with that config**: `nestrs run lint` does,
  and a project that runs no clippy, or predates the file, compiles a fragment that
  widens a filter. Nothing refuses one at compile time or at run, for the reasons above.
- **What no call writes.** Clippy names calls and types, so two spellings pass it, each
  pinned by a test in `select.rs`: a `sea_query::Enum` literal through its full path or
  a glob, which `disallowed-types` does not see, and a hand-written `impl Iden` whose
  `quoted` answers `Cow::Borrowed`, which writes that `'static` text unescaped. Each is
  one grep in the multi-tenant checklist.
- **A derive's expansion.** `#[sea_orm(select_as, save_as, save_array_as)]` expands to
  `ExprTrait::cast_as`, and a `DeriveActiveEnum` with `db_type = "Enum"` and no
  `rs_type` to `sea_query::Enum`, `OptionEnum::None` and `ArrayType::Enum`, beside the
  item carrying the derive. An `#[expect]` on that item stays unfulfilled and leaves
  the error, one on the module holding it opens it, `#![expect(…)]` at the top of its
  file, and `rs_type = "String"` on that enum needs none (`select.rs`'s tests and the
  scaffold's). The entries stay, since each splices. The demo and the scaffold spell
  neither, so everyday code pays nothing, and a project spelling one pays one attribute
  or one key.
- **Off the list, each read:** `Func::Custom`, since `FunctionCall::new` is `pub(crate)`
  and nothing public takes it to a statement; `raw_query!`, `raw_sql!`,
  `Statement::from_string` and the rest of the raw statement family, since inside a unit
  nothing can run a statement (`Repo` hands out no connection), and a contextless path
  holding `Arc<DatabaseConnection>` is its own named exception; `ColumnDef::custom`,
  `ColumnType::custom`, `extra` and `IndexType::Custom`, which are DDL; `ExplainStatement`'s
  `into_variable` and `table_with_wildcard`, MySQL's, on a statement no `Expr` holds.
- **What a select no longer offers**: `lock_exclusive`, `lock_shared`, `group_by`,
  `distinct`, and an order by an expression. None has a caller in the tree, and each is
  an inherent method, additive, when one comes.

## 2026-11-07 — a condition stays one operand, and text leaving it is refused

Under the owner's words of 2026-11-07 (take up every subject the open list names, by
the best solution, security first), the 2026-11-05 entry's "a raw fragment stays
clippy's" is replaced. Clippy held it only where an app runs clippy with that file, and
two spellings passed it whatever the app ran: a `sea_query::Enum` literal through its
full path and a hand-written `impl Iden`, each read every row on Postgres (the B-196
runs). A third shape needed no closing at all: text holding an `OR` inside an operand
sea-query writes bare, a column or a value's type name, read past the scope's `AND` by
precedence (`(1 = 0) AND "id" OR 1 = 1 OR "id" IS NOT NULL`).

`RepoSelect::filter`, and a page's and a relation page's `extra`, now render the
condition alone with sea-query's own Postgres writer, its values left as placeholders,
and scan that text; text passing is pushed as one `Expr::Tuple`, one pair of
parentheses sea-query writes around it, so the statement reads `scope AND (text)`. The
scan reads only what decides whether text stays inside that pair: every `(` closes
inside it and no `)` closes what it did not open, every `'…'`, `E'…'` and `"…"` closes,
and there is no comment, no dollar quoting, no `$` but a placeholder, no backslash in a
plain string and no NUL. Text failing makes the select build `FALSE` and its run answer
`DbErr::Custom` before it reads an executor, said at `warn` on `nest_rs::orm` by entity
and `reason`, never by its text. An order by field and a write's values (update, soft
delete, both unscoped inserts, `Creatable`, each after its hook) refuse an enum value,
alone or at any depth of an array, whose type name holds a `"`, the one text a bound value or an inline one writes beside
the quotes sea-query puts around it; a key cannot hold one, sea-orm reading no key into
a `sea_query::Enum`.

Why it holds where the four refusals of 2026-11-05 did not:

- **Parentheses at the select** failed alone, a fragment counting what to close. With
  the scan, a fragment cannot close the pair: its own text balances.
- **Scanning the rendered text, "a SQL lexer of our own, and a miss is silent"**: the
  scan reads no grammar, only the lexical regions Postgres's own lexer defines, its ten
  start conditions (`src/backend/parser/scan.l`). `xq`, `xqs`, `xus`, `xb` and `xh`
  close at a lone `'`, `xe` at a lone `'` with each backslash escaping the next byte,
  `xd` and `xui` at a lone `"`; `xc` (and `--`) and `xdolq` are refused, as is a
  backslash in `xq`, whose reading `standard_conforming_strings` decides. Its token
  boundaries are scan.l's: `$` continues an identifier, `E'` opens `xe` only where `E`
  is a token of its own. A region it does not read, it refuses, so a miss fails loud.
  Postgres's lexer is the standard it reads off, not one invented.
- **A walk of the tree** still stops at `CaseStatement` and a sub-select; the text is
  what reaches Postgres, every splice in it (`Iden`, type names, `BinOper::Custom`).
- **A derived table** is not needed: the operand holds an `OR`, and `UNION` needs a `)`.

The condition renders twice, alone for the scan and again in the statement, and both
read one tree: `filter` moves the expression it scanned into the statement, and every
text in it was fixed when it was built. sea-query 1.0 reads an `impl Iden` once, as the
expression is built (`From<T: Iden> for DynIden` stores `iden.quoted()`), and keeps a
custom fragment, an operator and a type name as strings; the two renders differ only in
the placeholders' numbers, which the scan reads alike.

Cost, measured in release on a three-clause condition (a column equality, a `LIKE`, an
`any` of two): the check takes about 0.56 µs beside the statement's own 0.79 µs render,
a fraction of a percent of a round trip. The scope and a key `Repo` builds itself go
through a crate-private `within` and pay nothing, and an empty `all` adds nothing.

The clippy lists stay as a second rung: they say it where it is written, at compile
time, where the scan says it when the query runs. Their reasons now say so.

An ability's rules write text into the scope too, and nothing parenthesizes a rule there:
the scope is the rules' conditions side by side, their denials and the tombstone's beside
them. A rule's typed `when` writes the entity's columns and bound values, of which a
value's enum type name is written as it is; and `PredicateBuilder::related` folds in the
relation's `on_condition`, any `Condition` an entity's `#[sea_orm(on_condition = …)]` or a
hand-written `RelationDef` writes, raw text included. Seen in review: an `on_condition` of
`1 = 1)) OR ((1 = 1` closed the rule's own subquery and read every row but the one a
denial named. The builder now holds each text where it is written, as `filter` does:
`related` lowers the `on_condition` once, scans it alone and folds it in as one
parenthesized operand, so a balanced `OR` it writes bare widens nothing beside it; a value
whose type name holds a `"` is refused. A rule failing either fails
`AbilityBuilder::build` with a `MalformedRuleError` naming the `reason`, one `error` on
`nest_rs::authz` beside it, and the guard answers `500`, as for a relation `related`
rejects; a denial left out would fail open. A scan of a rule's whole rendered text would not
do: a fragment balanced against the subquery around it passes one (tried before this was
written). It costs nothing per query: a typed rule renders nothing, the builder reading its
values, and an `on_condition` renders once, alone, as its rule is built. Measured in release
(200,000 builds, three rounds, two passes each): four typed rules, one through a relation,
build in about 1.06 µs before and after; with a fifth whose relation holds an
`on_condition`, 1.68 µs before and 1.67 to 1.71 µs after, within the rounds' spread.

What it leaves, kept and listed among the limits kept on purpose: the text an entity's own
definition writes, an `impl Iden` written by hand to answer its name borrowed, which
sea-query writes unescaped, or a `save_as` cast, is read as its schema, in a rule's scope as
in every statement `Repo` writes for that entity. `DeriveEntityModel` writes neither
unescaped. Closing it takes checking each identifier and cast an entity writes, once per
entity.

**Refused:**

- **`filter` answering `Result`**: every chain would take a `?`, everyday code harder
  (the owner's rule of 2026-11-05). The refusal travels in the select to its run, which
  answers `Result` already.
- **Closed comments and dollar quotes read as Postgres reads them**: nesting `/* */`,
  matching `$tag$` and the token boundaries around them are what a scan reads wrong
  silently, and a condition needs neither.
- **Scanning the scope a query reads**: a second render on every scoped query, for text
  each rule's builder checks once.

## 2026-11-11 — what `Repo` answers is the framework's error, said by the server's names

`Repo` handed out sea-orm's `DbErr`, and so did every caller of it: `RepoSelect`, the page
reads, `CrudService` and its write traits, `retry_on_conflict`, `ServiceError::Db`, and the
`AtomicError::Database` and `OutboxError::Database` the driver boxed. A `DbErr` prints
the server's words: run this date against a Postgres double sending what Postgres sends,
an insert meeting a unique key printed `detail: Some("Key (body)=(…) already exists.")`
through its `Debug`, and a read whose value a type refused printed `invalid input syntax for
type uuid: "…"` through its `Display`. `error_message` said both by their SQLSTATE, so no
framework line or reply quoted one; an app's own `{:?}`, `anyhow` report, `main` or
migration binary did.

Every one of them now answers `RepoError`, which holds the SQLSTATE and the names the
server reported (`Diagnostics`: `sqlstate`, `constraint`, `table`, `column`) and never its
message or `detail`. Its variants are what a caller branches on — `NotFound`,
`Forbidden { columns }`, `UniqueViolation`, `ForeignKeyViolation`,
`SerializationFailure` (`40001`, and `40P01`, a deadlock), `QueryCanceled` (`57014`), `Other`
— read off PostgreSQL's condition names where the server raised it, and the class every
edge answers where the framework did. `?` into `ServiceError` is unchanged; a service
deciding on a key matches it, `Err(RepoError::UniqueViolation(key)) if key.constraint() ==
Some("users_email_key")`. A `DbErr` a caller holds of its own — a migration, a seed, a
transaction it opened — becomes one with `RepoError::from`, which the demo's and `nestrs
new`'s migrate and seed binaries, their migrations crate's `migrate` helper and
`EphemeralDatabase` do.

A create its scope refuses is `Forbidden` naming no column, through a service's `?` as
through `#[crud]`: a service's create answered an opaque `500` and an `error` line for that
denial, seen on the merge base, where `#[crud]` answered `403`. sea-orm's own
`RecordNotInserted` stays opaque, since an `ON CONFLICT DO NOTHING` raises it too.

**Refused:**

- **`DbErr` kept, said through a view as `ServiceError::Db` was**: the field itself is what
  a `match` hands an app, and it prints the row.
- **A wrapper holding the `DbErr` privately, with a `kind()`**: a caller branches by a
  method and a second enum, where a variant is one `match`.
- **`DbError`** (one letter from `DbErr`, two concepts) and **`DatabaseError`** (sqlx's
  trait, which the transactions page names) as the name; `RepoError` sits beside
  `ServiceError`, `AtomicError` and `OutboxError`, each named for what answers it.
- **`DbOutcome` kept public**: two ways to tell a class, and it read a `DbErr`.
- **A variant per integrity condition now** (`23502`, `23514`, `23P01`): no caller branches
  on one; `<Condition>Violation(Diagnostics)` places each when one does (`TODO.md`).
