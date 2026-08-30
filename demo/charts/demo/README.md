# demo

The [NestRS](https://nestrs.dev) demo on Kubernetes: one image, every app.

The [Dockerfile](../../Dockerfile) builds every binary of the demo workspace
into a single runtime image, so an app here is a Deployment whose `command`
names its binary. Adding one is an entry in `apps`, not a template.

```bash
helm install demo demo/charts/demo \
  --set secrets.SEAORM__URL="postgres://user:pass@db:5432/nestrs" \
  --set secrets.REDIS__URL="redis://redis:6379"
```

`helm template` renders offline against Kubernetes 1.20 unless told otherwise,
below this chart's floor — pass `--kube-version 1.31.0` when rendering locally.

## What it deploys

| App | Kind | Port | Serves |
|---|---|---|---|
| `migrate` | job | — | the schema, before any deployment is updated |
| `api` | deployment | 3002 | REST, GraphQL, OpenAPI |
| `auth` | deployment | 3001 | the OAuth issuer and social login |
| `assistant` | deployment | 3003 | MCP, as an OAuth protected resource |
| `live` | deployment | 3004 | WebSocket |
| `worker` | deployment | 3005 | the queues (health only over HTTP) |

`seed` is not in the image. It writes demo fixtures, which a production image
has no business being able to do; it stays a local command (`nestrs run db seed`).

## Configuration

The framework reads its settings from the environment, so a cluster only ever
sets variables — `.env` is a local convenience the image does not carry.

- `config` — non-secret keys, spelled **without** the prefix, written to a
  ConfigMap. `LOG: info` becomes `NESTRS_LOG`.
- `secrets` — same spelling, written to a Secret this chart owns. Bring your own
  with `existingSecret` instead; the two are alternatives, not layers.
- `envPrefix` — renames every framework variable at once. It is set on the
  process, which is why it is a pod env var and never a file.

**Set the hostnames rather than the URLs.** The demo's apps address each other
by URL — the issuer a token is signed by, the audience it is minted for, the
resource identifier RFC 9728 discovery serves, the MCP `Host` allowlist, the
social redirect URIs — and every one of those is some app's public origin. The
chart derives them from `apps.<name>.host`, so renaming a hostname cannot leave
half the pair behind. Anything in `config` still wins.

## Scaling the queue worker

**Queue depth is the right signal, and KEDA is how you read it.** Two facts
from the framework's own source decide this:

1. The worker runs `concurrency(1)` per `#[process]` method — throughput comes
   from replicas, never from a bigger pod — and the demo's processors are pure
   I/O (an S3 round-trip, one INSERT). A pod draining a backlog of thousands
   sits near 0% CPU, so a CPU-driven HPA never fires. `autoscaling` is there for
   a processor that is genuinely CPU-bound; on these jobs it is inert.
2. Kubernetes has no native queue-depth trigger. HPA reads CPU, memory, or an
   external metric that something else must publish — and `HPAScaleToZero` was
   alpha from 1.16 to 1.36. [KEDA](https://keda.sh) is the one moving part that
   turns a list length into replicas.

```yaml
keda:
  redis:
    address: redis:6379   # KEDA dials Redis itself, and wants host:port
apps:
  worker:
    keda:
      enabled: true
```

The triggers ship pre-wired to the demo's two queues. The key names are apalis's
own layout — `<queue>:active` is the pending list, and `<queue>` is the name the
`#[queue]` declaration gives it.

`autoscaling` and `keda` on the same app is a render error: KEDA owns an HPA of
its own, and two of them would scale the same Deployment against each other.

### Why `minReplicaCount` is 1

Scale-to-zero is one value away and deliberately not the default:

- a worker start re-enqueues orphaned jobs across **all** registered consumers,
  not just its own previous incarnation, so every scale-up re-runs its peers'
  in-flight jobs. `#[process]` handlers must be idempotent — the framework says
  so already — and scaling stays unhurried on purpose;
- at zero replicas nothing promotes `<queue>:scheduled` into `<queue>:active`
  and nothing recovers `<queue>:inflight` from a pod that died mid-job. Neither
  is reachable from today's demo, where the producer pushes straight to
  `:active` and retries are in-process, but both become permanent the day a
  delayed push appears.

Set it to 0 knowing that.

## Graceful shutdown

The framework installs a SIGTERM handler and drains its transports, so
`terminationGracePeriodSeconds` is the window it gets. Two are not the default:
the worker's is 45s, above the 30s
`NESTRS_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS` it drains within, and `live`'s is
60s because a rollout drops WebSocket connections and clients reconnect.
