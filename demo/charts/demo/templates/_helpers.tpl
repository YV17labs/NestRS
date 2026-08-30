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

{{- define "demo.secretName" -}}
{{- default (printf "%s-env" (include "demo.fullname" .)) .Values.existingSecret }}
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
audience a token is minted for, the resource identifier RFC 9728 discovery
serves, the MCP Host allowlist, the social redirect URIs — and every one of them
is the same string as some app's public origin. Deriving them is what keeps a
rename of one hostname from silently breaking the pair.

`.Values.config` is laid over the result, so any of these can still be pinned.
*/}}
{{- define "demo.derivedConfig" -}}
{{- $auth := include "demo.origin" (dict "ctx" . "app" "auth") -}}
{{- $assistant := include "demo.origin" (dict "ctx" . "app" "assistant") -}}
{{- $derived := dict -}}
{{- if $auth -}}
{{- $_ := set $derived "AUTHN__ISSUER" $auth -}}
{{- $_ := set $derived "OAUTH_RESOURCE__AUTHORIZATION_SERVERS" $auth -}}
{{- $_ := set $derived "SOCIAL__GITHUB__REDIRECT_URL" (printf "%s/social/github/callback" $auth) -}}
{{- $_ := set $derived "SOCIAL__GOOGLE__REDIRECT_URL" (printf "%s/social/google/callback" $auth) -}}
{{- end -}}
{{- if $assistant -}}
{{- $_ := set $derived "AUTHN__AUDIENCE" $assistant -}}
{{- $_ := set $derived "OAUTH_RESOURCE__RESOURCE" $assistant -}}
{{- $_ := set $derived "MCP__ALLOWED_HOSTS" (index .Values.apps "assistant").host -}}
{{- end -}}
{{- $derived | toYaml -}}
{{- end }}

{{/*
Everything that lands in the ConfigMap: the derived settings, then the operator's
own `config` over them, every key wearing the deployment's prefix.
*/}}
{{- define "demo.plainConfig" -}}
{{- $merged := mergeOverwrite (fromYaml (include "demo.derivedConfig" .)) (deepCopy .Values.config) -}}
{{- range $key, $value := $merged }}
{{ printf "%s_%s" $.Values.envPrefix $key }}: {{ $value | quote }}
{{- end }}
{{- end }}

{{/*
The env of one app: the prefix itself (the one name no prefix renames), the
environment, the port this chart owns, then the ConfigMap and Secret wholesale.
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
{{- with $cfg.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{- define "demo.envFrom" -}}
- configMapRef:
    name: {{ include "demo.configMapName" .ctx }}
- secretRef:
    name: {{ include "demo.secretName" .ctx }}
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
that installs cleanly and then misbehaves in a way no probe reports.
*/}}
{{- define "demo.validate" -}}
{{- if and .Values.existingSecret .Values.secrets -}}
{{- fail "demo: set either `secrets` or `existingSecret`, not both — they are alternatives, and a chart-owned Secret would shadow yours." -}}
{{- end -}}
{{- if not .Values.existingSecret -}}
{{- if not (index .Values.secrets "SEAORM__URL") -}}
{{- fail "demo: every app needs a database. Set `secrets.SEAORM__URL`, or point `existingSecret` at a Secret that carries it." -}}
{{- end -}}
{{- end -}}
{{- range $app, $_ := .Values.apps -}}
{{- $cfg := fromYaml (include "demo.appConfig" (dict "ctx" $ "app" $app)) -}}
{{- if not (has $cfg.kind (list "deployment" "job")) -}}
{{- fail (printf "demo: apps.%s has kind %q. An app is either a `deployment` — a long-running process behind a Service — or a `job`, run to completion before them." $app $cfg.kind) -}}
{{- end -}}
{{- if and (eq $cfg.kind "deployment") (not $cfg.port) -}}
{{- fail (printf "demo: apps.%s needs a `port`. Every app the framework runs mounts an HTTP transport — the worker included, for its probes — and this chart sets NESTRS_HTTP__PORT from that value." $app) -}}
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
{{- end }}
