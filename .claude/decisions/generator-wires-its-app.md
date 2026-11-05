# A generator wires the app that serves its output, from anywhere in the workspace (7.0)

The identity audit of 2026-10-28 (new-3, read-12) found the generators wiring
an app only when run from inside `apps/<x>/`, which the tutorial told the
reader not to do, and, even there, printing what the generated code needed
the root to import: the data layer under a `#[crud]` handler, a queue's
backend. The owner agreed to the change with the audit's other findings
(2026-10-28, "Tout faire").

Adopted:

- **The app is resolved, not read off the working directory alone**: the one
  `--app <name>` names, else the one the directory is in, else the workspace's
  only app. The directory tree stays the configuration — `--app` takes the
  names under `apps/`, as `nestrs run dev <app>` does.
- **With several apps and none named, the feature is written and nothing is
  wired**: the run lists the apps and every import to write.
- **One edit writes every import the generated code needs that the root does
  not already name** — the edge; `SeaOrmModule` and `SeaOrmDatabaseModule`
  under a `#[crud]` handler; `RedisQueueModule` under a processor, with the
  `RedisModule` instance it reads; `AuthnHttpModule` when the run wrote the
  auth adapter. A root naming one — bare, pinned or a named instance — keeps
  its own line, so the root still holds every choice it makes. The one
  resource a binding reads is the exception: the bare `RedisQueueModule`
  reads Redis's default instance, so a root opening only a named one
  (`RedisModule::<Cache>`) gains the default beside it, rather than a binding
  that fails the boot on a connection nothing opens.

Refused:

- **Refusing the generation when the app is ambiguous**: the feature does not
  depend on which app serves it, and a refused run loses the work for a
  question one flag answers.
- **A project file naming the default app**: a second place for what the tree
  already says, which a renamed app leaves stale.
- **Wiring every app of the workspace**: an app imports exactly the edges it
  serves (`modular-monolith-per-workload.md`).
- **Printing a backend binding for the developer to add**: a step the
  generator could take, in the file it is already editing.

Held by `nest-rs-cli`'s integration suite (each generator from the root, with
one app, several, `--app`, a pinned data layer) and the e2e replay of the
tutorial, which runs every generator from the workspace root.

## 2026-10-31 — the table's migration is a step the generator takes

The identity audit's second pass (new-3, read2-6, new-6) found the generators
contradicting each other: `g resource` wrote a `name` column, the migration
`g migration` wrote created none, and the round trip posted it, so the
unedited scaffold answered 500; every column was typed four times in three
files; and `g entity` refused a `#[subject]` `g graphql|mcp` had written,
handing back three steps over the CLI's own files. The owner agreed with the
audit's twenty items (2026-10-31, "Tout faire (Recommandé)").

Adopted:

- **One column list, `<name>:<type>[:nullable]`**, the field grammar Rails'
  and Phoenix's generators share, writes the entity's fields, the table's
  migration — its `ColumnDef`s and `DeriveIden` variants — and the round trip
  that posts and reads back each column, in `g resource`'s one transaction;
  with none, one `name:string` column runs through all three.
- **`g migration create_<table>` reads the entity declaring that table**, its
  `Model`'s fields and the default its key takes as SeaORM reads it — an
  identity for an auto-incremented integer, `uuidv7()` for a `Uuid` key no
  `#[expose]` seeds, a key over several columns the table's — and names any
  it cannot type rather than guessing.
- **A migration name registered already is refused**, as Rails' generator
  refuses it, and so is a `create_<table>` whose table a registered one
  creates, however it is spelled (`create_post` after `create_posts`): under
  `if_not_exists()` its create-table would find the table and change nothing,
  a change that silently never lands. `g resource` over such a table is
  refused with its own way through — the migration removed, or the feature
  and its entity written alone.
- **`g entity` retires the subject the generators wrote** — the file, its two
  lines in `mod.rs`, each operation re-gated on the entity, which fails
  closed — and refuses, naming the file, one edited beyond that.

Refused:

- **Writing the entity's grant into `ability.rs` on a retirement**: an opening
  is the developer's line, never the generator's; it is printed.
- **`ab` in place of `_ab` in the empty policy, under
  `#[expect(unused_variables)]`**: the first grant leaves the expectation
  unfulfilled, which the scaffold's own `clippy -D warnings` refuses, so the
  edit moves rather than goes; `_ab` is the honest mark of an empty policy.
- **`?` as the nullable marker** (`bio:text?`): zsh, macOS's shell, reads it
  as a glob and refuses the word unquoted. The third segment is Rails'
  modifier slot, and `nullable` SeaORM's and Laravel's word for it.
- **A `text` column with no `column_type = "Text"`**, as the demo's `posts`
  spells it: `g migration` could then read the column back only as a
  `string`, and two generators would disagree on one column.

Held by `nest-rs-cli`'s integration suite (the column grammar and its
refusals, the migration `g resource` writes, the `Model` `g migration` reads,
the retirement and its refusal), its scaffold suite (every column type and an
entity's migration compiled under the scaffold's `clippy -D warnings`) and the
e2e replay of every tutorial page that edits `blog`, which runs an every-type
resource's round trip against the live Postgres.

## 2026-11-05 — the exception goes: a binding imports the resource it reads

`SeaOrmDatabaseModule` and each Redis binding now import their resource bare
(`adapter-imports-its-transport.md`, 2026-11-05), agreed with the second audit's
twenty items (the owner's "Tout faire (Recommandé)", 2026-10-31). So *the one
resource a binding reads is the exception* above is gone with its reason: there
is no binding left to fail the boot on a connection nothing opens. The generators
write `SeaOrmDatabaseModule` under a `#[crud]` handler and `RedisQueueModule`
under a processor, alone, and the scanner that read a root to tell whether it
opens the default instance went with the pair it wrote. A root's own lines stay,
a pinned resource among them, so the root still holds every choice it makes.
