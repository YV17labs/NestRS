# Each demo app holds its own secrets, and the chart refuses what an app could not run on

On 2026-11-08 the demo chart stopped writing one Secret every app read. Each app,
the migration job included, has a Secret of its own holding the keys of `secrets`
its `reads` names (`apps.<app>.reads` in `values.yaml`, each key `required` or
`optional`), and the chart refuses an install that leaves a `required` setting
unhanded to an app reading it, or a key of `secrets` reaching no app. The owner's
word of 2026-11-07 asked for every gap of `TODO.md` closed, security first; the
shared Secret handed `auth`'s registered clients to four apps that never read them.

- **`reads` lives in the values, read off each app's `module.rs`.** One map routes
  a secret and states the requirement, so a developer whose module starts reading
  a key adds one line, in the file an operator already edits, and no template
  moves.
- **`existingSecret` is per app**, `defaults.existingSecret` keeping one Secret for
  every app. An app with one is checked for nothing: the chart cannot read a
  Secret it does not write.
- **What the chart sees of an app** is its routed secrets, the shared `config`, its
  own `config` and the names its `extraEnv` sets, each under its name or as
  `<KEY>_FILE`; the social redirect derivation reads the same set, so a client id
  in `auth`'s `extraEnv` derives its redirect. An app refuses a setting set under both
  spellings, so a pin as `<KEY>_FILE` drops what the chart derives for `<KEY>`, and a
  setting an app is handed both ways refuses the install.
- **One `checksum/secret` over every Secret the chart writes**, on each pod reading
  one: a pod is read where a Secret is not (Kubernetes' `view` role), and a checksum of
  one app's Secret alone is an offline test of a guess at it — `live`'s, the database
  URL beside a public key, gave a dictionary the database password. A key changed rolls
  every app reading a chart-written Secret, as 0.6.0's one Secret did. Its limit: an
  install whose chart-written Secrets hold one weak secret and nothing else, every other
  app on an `existingSecret`, still publishes a checksum of that secret alone
  (`TODO.md`).
- **The render cases are a script** (`demo/charts/demo/ci/render.sh`) over `helm
  template` and `yq`, run by `nestrs run chart` and by `demo.yml`'s `chart` job.

**Refused:**

- **The shared Secret with `auth`'s keys handed through `extraEnv`**, the way out
  0.6.0 documented: least privilege took an operator's effort, and the default
  leaked.
- **The values per app** (`apps.<app>.secrets: {KEY: value}`): the database URL,
  which six apps read, typed six times.
- **Routing by namespace** (`AUTHN__*` to every app verifying a token): the
  signing key and the verifying key share `AUTHN__`, and only `auth` may hold the
  first.
- **A routing table inside the templates**: a key a developer's own module reads
  would need a template edit to reach it.
- **`lookup` to read an `existingSecret`**: it needs the cluster at render, and
  reads nothing under `helm template` or a client dry run.
- **A checksum per app**, so a key rolls only the apps reading it: an unsalted hash of
  one app's keys, on pods `view` reads. **A salt** to keep it: nothing a viewer cannot
  read is stable across renders without the cluster — a random one rolls every pod at
  each render, GitOps' included, and a stored one needs `lookup`.
- **A key of `secrets` no app reads, dropped**: a setting the operator wrote that
  reaches nothing is a swallowed one.
- **helm-unittest**: a plugin to install on every machine and runner, for cases a
  script over `helm template` and `yq` reads as plainly.
