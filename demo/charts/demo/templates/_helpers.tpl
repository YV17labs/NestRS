{{/*
Names and labels — the standard Helm shapes.
*/}}
{{- define "demo.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "demo.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "demo.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "demo.labels" -}}
helm.sh/chart: {{ include "demo.chart" . }}
{{ include "demo.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "demo.selectorLabels" -}}
app.kubernetes.io/name: {{ include "demo.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Per-app labels. `ctx` is the root context, `app` the key in `.Values.apps`.
The app is a component of one release, so it shares the release's selector plus
its own `component` — that pair is what a Deployment selects on.
*/}}
{{- define "demo.appSelectorLabels" -}}
{{ include "demo.selectorLabels" .ctx }}
app.kubernetes.io/component: {{ .app }}
{{- end }}

{{- define "demo.appLabels" -}}
{{ include "demo.labels" .ctx }}
app.kubernetes.io/component: {{ .app }}
{{- end }}

{{- define "demo.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "demo.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "demo.configMapName" -}}
{{- printf "%s-env" (include "demo.fullname" .) }}
{{- end }}

{{- define "demo.image" -}}
{{- printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) }}
{{- end }}

{{/*
One app's settings: `defaults` with the app's own entry laid over it.
Callers read it back with `fromYaml`.
*/}}
{{- define "demo.appConfig" -}}
{{- $app := index .ctx.Values.apps .app -}}
{{- mergeOverwrite (deepCopy .ctx.Values.defaults) (deepCopy $app) | toYaml -}}
{{- end }}

{{/*
The Secret one app reads: its own `existingSecret`, or the one the chart writes for it.
*/}}
{{- define "demo.appSecretName" -}}
{{- $cfg := fromYaml (include "demo.appConfig" (dict "ctx" .ctx "app" .app)) -}}
{{- default (printf "%s-%s" (include "demo.fullname" .ctx) .app) $cfg.existingSecret -}}
{{- end }}

{{/*
What the chart writes into one app's Secret: the keys of `secrets` its `reads` names,
and nothing when it brings its own. A key reaches only the apps reading it, so no app
holds another's credentials — `auth`'s signing key and registered clients above all, with
which a resource server could mint a token for any app. Callers read it back with
`fromYaml`.
*/}}
{{- define "demo.appSecrets" -}}
{{- $cfg := fromYaml (include "demo.appConfig" (dict "ctx" .ctx "app" .app)) -}}
{{- $data := dict -}}
{{- if not $cfg.existingSecret -}}
{{- $reads := default dict $cfg.reads -}}
{{- range $key, $value := default dict .ctx.Values.secrets -}}
{{- if and (hasKey $reads $key) (ne (toString $value) "") -}}
{{- $_ := set $data $key $value -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- $data | toYaml -}}
{{- end }}

{{/*
The settings a deployment sets on one app alone, unprefixed: its Secret's keys, its own
`config` and its `extraEnv`. An `existingSecret`'s keys are not among them: the chart
cannot read a Secret it does not write.
*/}}
{{- define "demo.appSets" -}}
{{- $cfg := fromYaml (include "demo.appConfig" (dict "ctx" .ctx "app" .app)) -}}
{{- $prefix := printf "%s_" .ctx.Values.envPrefix -}}
{{- $sets := dict -}}
{{- range $key, $_ := fromYaml (include "demo.appSecrets" .) -}}
{{- $_ := set $sets $key true -}}
{{- end -}}
{{- range $key, $value := default dict $cfg.config -}}
{{- if ne (toString $value) "" -}}
{{- $_ := set $sets $key true -}}
{{- end -}}
{{- end -}}
{{- range default list $cfg.extraEnv -}}
{{- if and (hasPrefix $prefix (toString .name)) (or .value .valueFrom) -}}
{{- $_ := set $sets (trimPrefix $prefix .name) true -}}
{{- end -}}
{{- end -}}
{{- $sets | toYaml -}}
{{- end }}

{{/*
The public origin of an app, or "" when it has no hostname.
*/}}
{{- define "demo.origin" -}}
{{- $host := (index .ctx.Values.apps .app).host -}}
{{- if $host -}}
{{- printf "%s://%s" .ctx.Values.scheme $host -}}
{{- end -}}
{{- end }}

{{/*
The settings the chart derives from the hostnames rather than having them typed
twice. The demo's apps address each other by URL — the issuer an app trusts, the
resource identifier RFC 9728 discovery serves, the resources a sign-in may be
minted a token for, the social redirect URIs — and every one of them is the
same string as some app's public origin. Deriving them is what keeps a rename of
one hostname from silently breaking the pair. The audience is no shared setting:
each app requires its own (`demo.appDerived`), and the assistant's MCP endpoint
admits its resource's host by itself. A social redirect is derived only for a
provider whose client id `auth` is handed where the chart sees it (`demo.appSets`,
or the shared `config`), under its name or as `<KEY>_FILE`: on its own a redirect
is a half-configured provider, which refuses `auth`'s boot.

`.Values.config` is laid over the result, so any of these can still be pinned, under
either spelling (`demo.sharedConfig`).
*/}}
{{- define "demo.derivedConfig" -}}
{{- $auth := include "demo.origin" (dict "ctx" . "app" "auth") -}}
{{- $assistant := include "demo.origin" (dict "ctx" . "app" "assistant") -}}
{{- $derived := dict -}}
{{- if $auth -}}
{{- $_ := set $derived "AUTHN__ISSUER" $auth -}}
{{- $_ := set $derived "OAUTH__RESOURCE__AUTHORIZATION_SERVERS" $auth -}}
{{- $readable := merge (fromYaml (include "demo.appSets" (dict "ctx" $ "app" "auth"))) (default dict $.Values.config) -}}
{{- range $provider := list "github" "google" -}}
{{- $namespace := printf "SOCIAL__%s" (upper $provider) -}}
{{- if or (index $readable (printf "%s__CLIENT_ID" $namespace)) (index $readable (printf "%s__CLIENT_ID_FILE" $namespace)) -}}
{{- $_ := set $derived (printf "%s__REDIRECT_URL" $namespace) (printf "%s/social/%s/callback" $auth $provider) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if $assistant -}}
{{- $_ := set $derived "OAUTH__RESOURCE__RESOURCE" $assistant -}}
{{- end -}}
{{- $resources := list -}}
{{- range $app := list "api" "assistant" "live" -}}
{{- if hasKey $.Values.apps $app -}}
{{- $origin := include "demo.origin" (dict "ctx" $ "app" $app) -}}
{{- if $origin -}}
{{- $resources = append $resources $origin -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if $resources -}}
{{- $_ := set $derived "SIGN_IN__RESOURCES" (join "," $resources) -}}
{{- end -}}
{{- $derived | toYaml -}}
{{- end }}

{{/*
What every app reads from the ConfigMap, unprefixed: the derived settings, then the
operator's own `config` over them. A pin as `<KEY>_FILE` drops the derived `<KEY>`,
which an app would refuse beside it. Callers read it back with `fromYaml`.
*/}}
{{- define "demo.sharedConfig" -}}
{{- $config := default dict .Values.config -}}
{{- $derived := fromYaml (include "demo.derivedConfig" .) -}}
{{- range $key, $value := $config -}}
{{- if and (hasSuffix "_FILE" $key) (ne (toString $value) "") -}}
{{- $_ := unset $derived (trimSuffix "_FILE" $key) -}}
{{- end -}}
{{- end -}}
{{- mergeOverwrite $derived (deepCopy $config) | toYaml -}}
{{- end }}

{{/*
Everything that lands in the ConfigMap, every key wearing the deployment's prefix.
*/}}
{{- define "demo.plainConfig" -}}
{{- range $key, $value := fromYaml (include "demo.sharedConfig" .) }}
{{ printf "%s_%s" $.Values.envPrefix $key }}: {{ $value | quote }}
{{- end }}
{{- end }}

{{/*
The settings the chart derives for one app alone, unprefixed: the audience a token must
carry to be accepted here — this app's own origin, so a token minted for one app is
refused by every other (RFC 8707) — unless the app is handed one under either spelling.
Callers read it back with `fromYaml`.
*/}}
{{- define "demo.appDerived" -}}
{{- $origin := include "demo.origin" (dict "ctx" .ctx "app" .app) -}}
{{- $pins := merge (fromYaml (include "demo.appSets" .)) (default dict .ctx.Values.config) -}}
{{- $derived := dict -}}
{{- if and $origin (not (or (hasKey $pins "AUTHN__AUDIENCE") (index $pins "AUTHN__AUDIENCE_FILE"))) -}}
{{- $_ := set $derived "AUTHN__AUDIENCE" $origin -}}
{{- end -}}
{{- $derived | toYaml -}}
{{- end }}

{{/*
The env of one app: the prefix itself (the one name no prefix renames), the
environment, the port this chart owns, what the chart derives for this app alone
(`demo.appDerived`) and the app's own `config`, each outranking the ConfigMap and
Secret that follow wholesale.
*/}}
{{- define "demo.env" -}}
{{- $cfg := fromYaml (include "demo.appConfig" (dict "ctx" .ctx "app" .app)) -}}
- name: NESTRS_ENV_PREFIX
  value: {{ .ctx.Values.envPrefix | quote }}
- name: {{ printf "%s_ENV" .ctx.Values.envPrefix }}
  value: {{ .ctx.Values.environment | quote }}
{{- if eq $cfg.kind "deployment" }}
- name: {{ printf "%s_HTTP__PORT" .ctx.Values.envPrefix }}
  value: {{ $cfg.port | quote }}
{{- end }}
{{- range $key, $value := fromYaml (include "demo.appDerived" .) }}
- name: {{ printf "%s_%s" $.ctx.Values.envPrefix $key }}
  value: {{ $value | quote }}
{{- end }}
{{- range $key, $value := $cfg.config }}
- name: {{ printf "%s_%s" $.ctx.Values.envPrefix $key }}
  value: {{ $value | quote }}
{{- end }}
{{- with $cfg.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{/*
Where one app's env comes from wholesale: the ConfigMap every app shares, then the
Secret holding what it alone reads, when it reads any.
*/}}
{{- define "demo.envFrom" -}}
- configMapRef:
    name: {{ include "demo.configMapName" .ctx }}
{{- include "demo.secretRef" . }}
{{- end }}

{{- define "demo.secretRef" -}}
{{- $cfg := fromYaml (include "demo.appConfig" (dict "ctx" .ctx "app" .app)) -}}
{{- if or $cfg.existingSecret (fromYaml (include "demo.appSecrets" .)) }}
- secretRef:
    name: {{ include "demo.appSecretName" . }}
{{- end }}
{{- end }}

{{/*
The probes. Identical at every app, worker included — it mounts an HTTP
transport for exactly this.
*/}}
{{- define "demo.probes" -}}
startupProbe:
  httpGet:
    path: /health/startup
    port: http
  periodSeconds: 2
  failureThreshold: 30
readinessProbe:
  httpGet:
    path: /health/ready
    port: http
  periodSeconds: 10
  failureThreshold: 3
livenessProbe:
  httpGet:
    path: /health/live
    port: http
  periodSeconds: 10
  failureThreshold: 3
{{- end }}

{{/*
What the chart refuses to render, and why. Every one of these is a deployment
that would install cleanly and then misbehave in a way no probe reports, or
whose apps would refuse to boot on what it renders (`demo.validateReads`).
*/}}
{{- define "demo.validate" -}}
{{- if hasKey .Values "existingSecret" -}}
{{- fail "demo: `existingSecret` is set per app, so that each app holds only what it reads: `apps.<app>.existingSecret` hands one app a Secret of yours, `defaults.existingSecret` hands every app the same one." -}}
{{- end -}}
{{- range $app, $_ := .Values.apps -}}
{{- $cfg := fromYaml (include "demo.appConfig" (dict "ctx" $ "app" $app)) -}}
{{- if not (has $cfg.kind (list "deployment" "job")) -}}
{{- fail (printf "demo: apps.%s has kind %q. An app is either a `deployment` — a long-running process behind a Service — or a `job`, run to completion before them." $app $cfg.kind) -}}
{{- end -}}
{{- if eq $cfg.kind "deployment" -}}
{{- if not $cfg.port -}}
{{- fail (printf "demo: apps.%s needs a `port`. Every app the framework runs mounts an HTTP transport — the worker included, for its probes — and this chart sets NESTRS_HTTP__PORT from that value." $app) -}}
{{- end -}}
{{- $wayDown := 29.5 -}}
{{- $needed := ceil (addf $cfg.shutdownDelaySeconds $wayDown) -}}
{{- if lt (float64 $cfg.terminationGracePeriodSeconds) $needed -}}
{{- fail (printf "demo: apps.%s has a terminationGracePeriodSeconds of %v, under the %v its way down needs. Kubernetes counts the preStop sleep inside the grace, so the %vs `shutdownDelaySeconds` comes first, then the framework's %vs from SIGTERM to exit at its defaults; a pod past its grace is killed mid-drain, with no line saying why. Raise the grace, or lower the delay." $app $cfg.terminationGracePeriodSeconds $needed $cfg.shutdownDelaySeconds $wayDown) -}}
{{- end -}}
{{- end -}}
{{- if and $cfg.autoscaling.enabled $cfg.keda.enabled -}}
{{- fail (printf "demo: apps.%s enables both `autoscaling` and `keda`. KEDA owns an HPA of its own, so the two would scale the same Deployment against each other — pick one." $app) -}}
{{- end -}}
{{- if $cfg.keda.enabled -}}
{{- if not $cfg.keda.triggers -}}
{{- fail (printf "demo: apps.%s enables `keda` with no triggers — a ScaledObject with none scales to its minimum and stays there." $app) -}}
{{- end -}}
{{- range $cfg.keda.triggers -}}
{{- if eq .type "redis" -}}
{{- if not (or $.Values.keda.redis.address (index .metadata "address") (index .metadata "addressFromEnv") (index .metadata "host")) -}}
{{- fail "demo: a redis trigger needs somewhere to connect. Set `keda.redis.address` to host:port — KEDA dials Redis from the operator, not through the pod, and its scaler takes host:port rather than the redis:// URL the framework reads." -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- include "demo.validateReads" . -}}
{{- end }}

{{/*
Each app's `reads`, held against what the chart renders for it: a `required` setting
nothing hands it, under its name or as `<KEY>_FILE`, and a social provider named in
part, refuse the install, since the app would refuse its boot on it (`auth` without its
signing key boots, and answers 500 at its token endpoint); so does a setting an app is
handed both under its name and as `<KEY>_FILE`. An app with an `existingSecret` is left
to it: the chart cannot read a Secret it does not write. A key of `secrets` no app reads
is refused too, rather than dropped.
*/}}
{{- define "demo.validateReads" -}}
{{- $problems := list -}}
{{- $needed := dict -}}
{{- $doubled := dict -}}
{{- $reached := dict -}}
{{- $shared := dict -}}
{{- range $key, $value := fromYaml (include "demo.sharedConfig" .) -}}
{{- if ne (toString $value) "" -}}
{{- $_ := set $shared $key true -}}
{{- end -}}
{{- end -}}
{{- range $app, $_ := .Values.apps -}}
{{- $cfg := fromYaml (include "demo.appConfig" (dict "ctx" $ "app" $app)) -}}
{{- $reads := default dict $cfg.reads -}}
{{- if not $cfg.existingSecret -}}
{{- $reached = merge $reached $reads -}}
{{- $seen := merge (fromYaml (include "demo.appSets" (dict "ctx" $ "app" $app))) (fromYaml (include "demo.appDerived" (dict "ctx" $ "app" $app))) $shared -}}
{{- $missing := list -}}
{{- range $key, $need := $reads -}}
{{- if and (eq $need "required") (not (or (hasKey $seen $key) (hasKey $seen (printf "%s_FILE" $key)))) -}}
{{- $missing = append $missing $key -}}
{{- end -}}
{{- end -}}
{{- range $provider := list "GITHUB" "GOOGLE" -}}
{{- $namespace := printf "SOCIAL__%s" $provider -}}
{{- if hasKey $reads (printf "%s__CLIENT_ID" $namespace) -}}
{{- $named := list -}}
{{- $unnamed := list -}}
{{- range $part := list "CLIENT_ID" "CLIENT_SECRET" "REDIRECT_URL" -}}
{{- $key := printf "%s__%s" $namespace $part -}}
{{- if or (hasKey $seen $key) (hasKey $seen (printf "%s_FILE" $key)) -}}
{{- $named = append $named $key -}}
{{- else -}}
{{- $unnamed = append $unnamed $key -}}
{{- end -}}
{{- end -}}
{{- if has (printf "%s__CLIENT_ID" $namespace) $unnamed -}}
{{- $unnamed = without $unnamed (printf "%s__REDIRECT_URL" $namespace) -}}
{{- end -}}
{{- if $named -}}
{{- $missing = concat $missing $unnamed -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- range $key := $missing -}}
{{- $_ := set $needed $key (append (default list (get $needed $key)) (printf "apps.%s" $app)) -}}
{{- end -}}
{{- range $key, $_ := $seen -}}
{{- if and (hasSuffix "_FILE" $key) (hasKey $seen (trimSuffix "_FILE" $key)) -}}
{{- $_ := set $doubled (trimSuffix "_FILE" $key) (append (default list (get $doubled (trimSuffix "_FILE" $key))) (printf "apps.%s" $app)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- range $key, $apps := $needed -}}
{{- $problems = append $problems (printf "%s, which %s cannot run without: %s" $key (join ", " $apps) (include "demo.hint" $key)) -}}
{{- end -}}
{{- $derived := fromYaml (include "demo.derivedConfig" .) -}}
{{- range $key, $apps := $doubled -}}
{{- $problems = append $problems (printf "%s, handed to %s both as %s and as %s_FILE, which an app refuses at boot: give it once%s." $key (join ", " $apps) $key $key (ternary ". The chart derives it, and yields to a pin in `config` under either spelling" "" (hasKey $derived $key))) -}}
{{- end -}}
{{- range $key, $_ := default dict .Values.secrets -}}
{{- if not (hasKey $reached $key) -}}
{{- $problems = append $problems (printf "secrets.%s reaches no app: an app reads a key of `secrets` only when its `reads` names it, and an app with an `existingSecret` reads that Secret alone. Name it under `apps.<app>.reads` of each app reading it, or drop it." $key) -}}
{{- end -}}
{{- end -}}
{{- if $problems -}}
{{- fail (printf "demo: an app would not run on what this release renders.\n  - %s" (join "\n  - " $problems)) -}}
{{- end -}}
{{- end }}

{{/*
Where a missing setting comes from: a hostname, for those the chart derives
(`demo.derivedConfig`, `demo.appDerived`); the other half, for a social provider named
in part; the operator's values for the rest.
*/}}
{{- define "demo.hint" -}}
{{- $derived := dict
  "SIGN_IN__RESOURCES" "`apps.api.host`, `apps.assistant.host` or `apps.live.host`"
  "OAUTH__RESOURCE__RESOURCE" "`apps.assistant.host`"
  "OAUTH__RESOURCE__AUTHORIZATION_SERVERS" "`apps.auth.host`"
  "AUTHN__ISSUER" "`apps.auth.host`" -}}
{{- if regexMatch "^SOCIAL__[A-Z]+__REDIRECT_URL$" . -}}
{{- $_ := set $derived . "`apps.auth.host`" -}}
{{- end -}}
{{- if eq . "AUTHN__AUDIENCE" -}}
{{- print "the chart derives each app's own from its `host`. Set that, or pin it in `apps.<app>.config`." -}}
{{- else if hasKey $derived . -}}
{{- printf "the chart derives it from %s. Set that, or pin it in `config`." (index $derived .) -}}
{{- else if hasPrefix "SOCIAL__" . -}}
{{- printf "a provider named in part refuses the boot. Set `secrets.%s` too, or drop the provider's other settings." . -}}
{{- else -}}
{{- printf "set `secrets.%s` (`config.%s` if it is no secret), or give each an `existingSecret` that carries it." . . -}}
{{- end -}}
{{- end }}
