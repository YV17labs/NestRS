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
  under a `#[crud]` handler; `RedisModule` and `RedisQueueModule` under a
  processor; `AuthnHttpModule` when the run wrote the auth adapter. A root
  naming one — bare, pinned or a named instance — keeps its own line, so the
  root still holds every choice it makes.

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
