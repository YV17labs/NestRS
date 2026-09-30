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

1. A `#[process]` method runs as many jobs at once as its `concurrency`
   declares, in each replica — four transcodes, one notification write — and
   throughput past that comes from replicas. Both jobs are pure I/O (an S3
   round-trip, one INSERT), so a pod draining a backlog of thousands sits near
   0% CPU and a CPU-driven HPA never fires. `autoscaling` is there for a
   processor that is genuinely CPU-bound; on these jobs it is inert.
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

The triggers ship pre-wired to the demo's two queues. Each names the list a
worker fetches from, `nestrs:queue:<queue>:active`, where `<queue>` is the name
the `#[queue]` declaration gives it. Its `listLength` is the backlog one replica
is sized for, so it moves with the method's `concurrency`: the audio trigger
asks for a replica per 20 waiting because each one runs four transcodes at once.

The trigger counts **waiting** jobs only. A fetch takes a job off `active`, so a
replica busy with long transcodes reads as idle once the list drains, and KEDA
may scale it down mid-job. The pod then drains: running jobs get the shutdown
window, and what still runs when it closes is handed back to the queue and runs
again, from the start, on another replica. Nothing is lost, but the work is
done twice. Bound it with a scale-down stabilization window at least as long as
a job — `keda.behavior`, rendered as the ScaledObject's
`advanced.horizontalPodAutoscalerConfig.behavior`:

```yaml
apps:
  worker:
    keda:
      behavior:
        scaleDown:
          stabilizationWindowSeconds: 600
```

— and keep `terminationGracePeriodSeconds` above
`NESTRS_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS`, raising both toward a job's
length when jobs are long (see [Graceful shutdown](#graceful-shutdown)).

`autoscaling` and `keda` on the same app is a render error: KEDA owns an HPA of
its own, and two of them would scale the same Deployment against each other.

Scaling the worker does not multiply its schedule. The one job it runs on a
clock, the hourly notifications purge, is declared `replicas = "one"`: each
occurrence is claimed in the Redis the queues already use and fires on one
replica, whatever `maxReplicaCount` allows.

### Why `minReplicaCount` is 1

Scale-to-zero is one value away and deliberately not the default:

- a job held back — a retry waiting for its backoff, a delayed push — waits on
  `nestrs:queue:<queue>:scheduled`, which the trigger does not read, and only a
  running worker, or the producer that filed it while it has delayed jobs
  ahead, moves it onto the list the trigger reads. Every demo job retries, so
  at zero replicas a retry waits for the next push to wake the deployment;
- at zero replicas nothing recovers the jobs a pod that died mid-job held,
  until a replica starts.

A scale-up is safe either way: a worker start puts its peers' in-flight jobs
back on the queue, and each job's lease and settled mark keep them from running
twice. Set it to 0 knowing that.

## Graceful shutdown

The framework installs a SIGTERM handler and drains its transports, so
`terminationGracePeriodSeconds` is the window it gets. Two are not the default:
the worker's is 45s, above the 30s
`NESTRS_REDIS__WORKER__SHUTDOWN_TIMEOUT_SECS` it drains within, and `live`'s is
60s because a rollout drops WebSocket connections and clients reconnect.
