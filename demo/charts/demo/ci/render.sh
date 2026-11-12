#!/usr/bin/env bash
# The chart's lint and render cases: what each app's Secret holds, and what the chart
# refuses at install because an app would not run on it. Needs helm 4 and yq 4 (mikefarah's).
set -euo pipefail

chart="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# A client dry run reads no cluster: no kubeconfig of the caller's is consulted.
export KUBECONFIG=/dev/null
failures=0

printf -- '-----BEGIN PRIVATE KEY-----\nstand-in\n-----END PRIVATE KEY-----\n' > "$work/signing.pem"
printf -- '-----BEGIN PUBLIC KEY-----\nstand-in\n-----END PUBLIC KEY-----\n' > "$work/verifying.pem"
printf '[{"client_id":"web","client_secret":"stand-in","payload":{},"scopes":[],"resources":[]}]' > "$work/clients.json"

hosts=(
  --set apps.auth.host=auth.example.com
  --set apps.api.host=api.example.com
  --set apps.assistant.host=assistant.example.com
  --set apps.live.host=live.example.com
)
# `@` reads the value from a file, as `--set-file` does.
secret_values=(
  SEAORM__URL=postgres://user:pass@db:5432/nestrs
  REDIS__URL=rediss://redis:6379
  REDIS__TRANSCODES__URL=rediss://cache:6379
  STORAGE__ACCESS_KEY=stand-in
  STORAGE__SECRET_KEY=stand-in
  "AUTHN__PRIVATE_KEY=@$work/signing.pem"
  "AUTHN__PUBLIC_KEY=@$work/verifying.pem"
  "OAUTH__SERVER__CLIENTS=@$work/clients.json"
)

# without <keys…>: the install line's secrets but these, as helm arguments, into $secrets.
# A key is left out of the line rather than set to null, which the schema refuses first.
without() {
  secrets=()
  local entry key value
  for entry in "${secret_values[@]}"; do
    key="${entry%%=*}"
    value="${entry#*=}"
    case " $* " in *" $key "*) continue ;; esac
    if [ "${value#@}" != "$value" ]; then
      secrets+=(--set-file "secrets.$key=${value#@}")
    else
      secrets+=(--set "secrets.$key=$value")
    fi
  done
}

without
line_secrets=("${secrets[@]}")
# The line the READMEs print.
install=("${hosts[@]}" "${line_secrets[@]}")

fail() {
  echo "FAIL $current: $*"
  failures=$((failures + 1))
}

render() {
  helm template demo "$chart" --kube-version 1.31.0 "$@" > "$work/out.yaml" 2> "$work/err.txt"
}

# renders <case> <helm args…>: the render succeeds; its output is $work/out.yaml.
renders() {
  current="$1"
  shift
  if render "$@"; then
    echo "ok   $current"
  else
    fail "refused: $(cat "$work/err.txt")"
    return 1
  fi
}

# refuses <case> <pattern> <helm args…>: the render fails, saying what the extended
# regular expression matches.
refuses() {
  current="$1"
  local pattern="$2"
  shift 2
  if render "$@"; then
    fail "rendered, expected a refusal saying: $pattern"
  elif grep -qE -- "$pattern" "$work/err.txt"; then
    echo "ok   $current"
  else
    fail "refused without saying: $pattern — $(cat "$work/err.txt")"
  fi
}

# words: stdin's lines, blanks and document separators dropped, sorted onto one line.
words() {
  sed -e '/^$/d' -e '/^---$/d' | sort | tr '\n' ' '
}

# holds <Secret> <keys…>: the last render's Secret holds exactly these keys, unprefixed.
holds() {
  local name="$1"
  shift
  local want got
  want="$(printf '%s\n' "$@" | words)"
  got="$(yq ea "select(.kind == \"Secret\" and .metadata.name == \"$name\") | .stringData | keys | .[]" "$work/out.yaml" \
    | sed 's/^NESTRS_//' | words)"
  [ "$got" = "$want" ] || fail "Secret $name holds [$got], expected [$want]"
}

# secrets_are <names…>: the last render's Secrets are exactly these.
secrets_are() {
  local want got
  want="$(printf '%s\n' "$@" | words)"
  got="$(yq ea 'select(.kind == "Secret") | .metadata.name' "$work/out.yaml" | words)"
  [ "$got" = "$want" ] || fail "Secrets [$got], expected [$want]"
}

# reads <kind>/<name> <Secret|->: the workload's envFrom names the ConfigMap and this Secret alone.
reads() {
  local kind="${1%%/*}" name="${1#*/}" secret="$2" got want
  got="$(yq ea "select(.kind == \"$kind\" and .metadata.name == \"$name\") | .spec.template.spec.containers[0].envFrom[] | (.configMapRef.name // \"\") + (.secretRef.name // \"\")" "$work/out.yaml" | sed -e '/^$/d' -e '/^---$/d' | tr '\n' ' ')"
  if [ "$kind" = Job ]; then want=""; else want="demo-env "; fi
  [ "$secret" = - ] || want="$want$secret "
  [ "$got" = "$want" ] || fail "$kind $name reads [$got], expected [$want]"
}

# annotation <Deployment> <key>: one annotation of the last render's pod template; empty when absent.
annotation() {
  yq ea "select(.kind == \"Deployment\" and .metadata.name == \"$1\") | .spec.template.metadata.annotations[\"$2\"] // \"\"" "$work/out.yaml"
}

# podenv <Deployment> <key>: one env entry of the last render's Deployment, unprefixed; empty when absent.
podenv() {
  yq ea "select(.kind == \"Deployment\" and .metadata.name == \"$1\") | .spec.template.spec.containers[0].env[] | select(.name == \"NESTRS_$2\") | .value" "$work/out.yaml"
}

# configmap <key>: one key of the last render's ConfigMap, unprefixed; empty when absent.
configmap() {
  yq ea "select(.kind == \"ConfigMap\") | .data[\"NESTRS_$1\"] // \"\"" "$work/out.yaml"
}

# needs <key> <app>: the pattern of the refusal's line naming a setting an app cannot run without.
needs() {
  echo "- $1, which [a-z., ]*apps\\.$2[ ,]"
}

store=(STORAGE__ACCESS_KEY STORAGE__SECRET_KEY)
redis=(REDIS__URL REDIS__TRANSCODES__URL)

# --- the install line lints, and each app holds only what it reads ------------------
current="helm lint on the install line"
if helm lint "$chart" --strict --kube-version 1.31.0 "${install[@]}" > "$work/lint.txt" 2>&1; then
  echo "ok   $current"
else
  fail "$(cat "$work/lint.txt")"
fi

if renders "the install line renders" "${install[@]}"; then
  secrets_are demo-api demo-assistant demo-auth demo-live demo-migrate demo-worker
  holds demo-api SEAORM__URL "${redis[@]}" "${store[@]}" AUTHN__PUBLIC_KEY
  holds demo-assistant SEAORM__URL "${redis[@]}" "${store[@]}" AUTHN__PUBLIC_KEY
  holds demo-auth SEAORM__URL AUTHN__PRIVATE_KEY AUTHN__PUBLIC_KEY OAUTH__SERVER__CLIENTS
  holds demo-live SEAORM__URL AUTHN__PUBLIC_KEY
  holds demo-worker SEAORM__URL "${redis[@]}" "${store[@]}"
  holds demo-migrate SEAORM__URL
  for app in api assistant auth live worker; do reads "Deployment/demo-$app" "demo-$app"; done
  reads Job/demo-migrate demo-migrate
  [ "$(configmap AUTHN__ISSUER)" = https://auth.example.com ] || fail "the issuer is not auth's origin"
  for app in api auth live; do
    [ "$(podenv "demo-$app" AUTHN__AUDIENCE)" = "https://$app.example.com" ] || fail "demo-$app's audience is not its own origin"
  done
  checksum="$(annotation demo-live checksum/secret)"
  [ -n "$checksum" ] || fail "live's pods carry no checksum/secret"
  for app in api assistant auth worker; do
    [ "$(annotation "demo-$app" checksum/secret)" = "$checksum" ] || fail "demo-$app's checksum/secret is not every pod's one"
  done
fi

# A pod's annotations are read where its Secret is not: a hash of live's Secret alone, the
# database URL beside a public key, would test a guess at the database password offline.
if renders "a secret live does not read rolls live too" "${install[@]}" --set secrets.STORAGE__SECRET_KEY=other; then
  [ "$(annotation demo-live checksum/secret)" != "$checksum" ] || fail "live's checksum hashes its own Secret alone"
fi
printf -- '-----BEGIN PRIVATE KEY-----\nanother\n-----END PRIVATE KEY-----\n' > "$work/other.pem"
if renders "the signing key is in every pod's checksum" "${install[@]}" --set-file "secrets.AUTHN__PRIVATE_KEY=$work/other.pem"; then
  for app in api assistant auth live worker; do
    [ "$(annotation "demo-$app" checksum/secret)" != "$checksum" ] || fail "demo-$app's checksum leaves the signing key out"
  done
fi

# --- each key an app requires refuses the install --------------------------------
for entry in \
  "SEAORM__URL:api assistant auth live worker migrate" \
  "REDIS__URL:api assistant worker" \
  "REDIS__TRANSCODES__URL:api assistant worker" \
  "STORAGE__ACCESS_KEY:api assistant worker" \
  "STORAGE__SECRET_KEY:api assistant worker" \
  "AUTHN__PRIVATE_KEY:auth" \
  "AUTHN__PUBLIC_KEY:api assistant auth live" \
  "OAUTH__SERVER__CLIENTS:auth"; do
  key="${entry%%:*}"
  for app in ${entry#*:}; do
    without "$key"
    refuses "no $key refuses $app" "$(needs $key $app)" "${hosts[@]}" "${secrets[@]}"
  done
done

refuses "no hostname refuses auth" "$(needs SIGN_IN__RESOURCES auth)" "${line_secrets[@]}"
refuses "no hostname refuses the assistant's resource" "$(needs OAUTH__RESOURCE__RESOURCE assistant)" "${line_secrets[@]}"
refuses "no issuer's hostname refuses the assistant" "$(needs OAUTH__RESOURCE__AUTHORIZATION_SERVERS assistant)" \
  "${install[@]}" --set apps.auth.host=
for app in api auth live; do
  refuses "no issuer's hostname refuses $app" "$(needs AUTHN__ISSUER $app)" "${install[@]}" --set apps.auth.host=
  refuses "no hostname of its own refuses $app's audience" "$(needs AUTHN__AUDIENCE $app)" \
    "${install[@]}" --set "apps.$app.host="
done
renders "an audience pinned on live stands for its hostname" "${install[@]}" --set apps.live.host= \
  --set apps.live.config.AUTHN__AUDIENCE=https://live.example.com || true
renders "pinned settings stand for the hostnames" "${line_secrets[@]}" \
  --set config.SIGN_IN__RESOURCES=https://api.example.com \
  --set config.OAUTH__RESOURCE__RESOURCE=https://assistant.example.com \
  --set config.OAUTH__RESOURCE__AUTHORIZATION_SERVERS=https://auth.example.com \
  --set config.AUTHN__ISSUER=https://auth.example.com \
  --set apps.api.config.AUTHN__AUDIENCE=https://api.example.com \
  --set apps.auth.config.AUTHN__AUDIENCE=https://auth.example.com \
  --set apps.live.config.AUTHN__AUDIENCE=https://live.example.com || true
refuses "an empty value is no value" "$(needs STORAGE__URL worker)" "${install[@]}" --set config.STORAGE__URL=
refuses "an empty secret is no secret" "$(needs REDIS__URL api)" "${install[@]}" --set secrets.REDIS__URL=

# --- a value the chart cannot see is no refusal ----------------------------------
without OAUTH__SERVER__CLIENTS
renders "a key handed through extraEnv is seen" "${hosts[@]}" "${secrets[@]}" \
  --set apps.auth.extraEnv[0].name=NESTRS_OAUTH__SERVER__CLIENTS \
  --set apps.auth.extraEnv[0].valueFrom.secretKeyRef.name=clients \
  --set apps.auth.extraEnv[0].valueFrom.secretKeyRef.key=clients.json || true
without AUTHN__PRIVATE_KEY
renders "a key read from a file is seen" "${hosts[@]}" "${secrets[@]}" \
  --set apps.auth.config.AUTHN__PRIVATE_KEY_FILE=/vault/secrets/signing.pem || true
if renders "a Secret of your own replaces the chart's for every app" "${hosts[@]}" --set defaults.existingSecret=mine; then
  secrets_are
  for app in api assistant auth live worker; do
    reads "Deployment/demo-$app" mine
    [ -z "$(annotation "demo-$app" checksum/secret)" ] || fail "demo-$app's pods roll on a Secret the chart does not write"
  done
  reads Job/demo-migrate mine
fi
renders "a blank secrets beside a Secret of your own" "${hosts[@]}" --set defaults.existingSecret=mine --set secrets=null || true
without AUTHN__PRIVATE_KEY OAUTH__SERVER__CLIENTS
auth_own=("${hosts[@]}" "${secrets[@]}" --set apps.auth.existingSecret=auth-own)
if renders "one app brings its own Secret" "${auth_own[@]}"; then
  secrets_are demo-api demo-assistant demo-live demo-migrate demo-worker
  reads Deployment/demo-auth auth-own
  holds demo-live SEAORM__URL AUTHN__PUBLIC_KEY
fi

# --- a key no app reads, and the old spelling --------------------------------------
refuses "a secret no app reads refuses" "secrets.SIGN_IN__TYPO reaches no app" "${install[@]}" --set secrets.SIGN_IN__TYPO=x
refuses "a secret only an app with its own Secret reads refuses" "secrets.OAUTH__SERVER__CLIENTS reaches no app" \
  "${install[@]}" --set apps.auth.existingSecret=auth-own
refuses "the chart-wide existingSecret is refused naming its place" "defaults.existingSecret" "${hosts[@]}" --set existingSecret=mine

# --- a social provider is whole or absent -----------------------------------------
if renders "a provider named whole is on" "${install[@]}" \
  --set secrets.SOCIAL__GITHUB__CLIENT_ID=id --set secrets.SOCIAL__GITHUB__CLIENT_SECRET=stand-in; then
  holds demo-auth SEAORM__URL AUTHN__PRIVATE_KEY AUTHN__PUBLIC_KEY OAUTH__SERVER__CLIENTS \
    SOCIAL__GITHUB__CLIENT_ID SOCIAL__GITHUB__CLIENT_SECRET
  holds demo-api SEAORM__URL "${redis[@]}" "${store[@]}" AUTHN__PUBLIC_KEY
  [ "$(configmap SOCIAL__GITHUB__REDIRECT_URL)" = https://auth.example.com/social/github/callback ] \
    || fail "no redirect derived for github"
  [ -z "$(configmap SOCIAL__GOOGLE__REDIRECT_URL)" ] || fail "a redirect derived for google, which nobody named"
fi
if renders "a client id in config is read" "${install[@]}" \
  --set config.SOCIAL__GOOGLE__CLIENT_ID=id --set secrets.SOCIAL__GOOGLE__CLIENT_SECRET=stand-in; then
  [ -n "$(configmap SOCIAL__GOOGLE__REDIRECT_URL)" ] || fail "no redirect derived for google"
fi
if renders "a client id handed through extraEnv is read" "${install[@]}" --set secrets.SOCIAL__GITHUB__CLIENT_SECRET=stand-in \
  --set apps.auth.extraEnv[0].name=NESTRS_SOCIAL__GITHUB__CLIENT_ID --set apps.auth.extraEnv[0].value=id; then
  [ -n "$(configmap SOCIAL__GITHUB__REDIRECT_URL)" ] || fail "no redirect derived for github"
fi
refuses "a client id without its secret refuses" "$(needs SOCIAL__GITHUB__CLIENT_SECRET auth)" \
  "${install[@]}" --set secrets.SOCIAL__GITHUB__CLIENT_ID=id
refuses "a client secret without its id refuses" "$(needs SOCIAL__GOOGLE__CLIENT_ID auth)" \
  "${install[@]}" --set secrets.SOCIAL__GOOGLE__CLIENT_SECRET=stand-in
refuses "a provider with no hostname to derive its redirect refuses" "$(needs SOCIAL__GITHUB__REDIRECT_URL auth)" \
  "${install[@]}" --set apps.auth.host= --set config.OAUTH__RESOURCE__AUTHORIZATION_SERVERS=https://auth.example.com \
  --set secrets.SOCIAL__GITHUB__CLIENT_ID=id --set secrets.SOCIAL__GITHUB__CLIENT_SECRET=stand-in
renders "a provider behind a Secret of auth's own is left to it" "${auth_own[@]}" \
  --set config.SOCIAL__GITHUB__REDIRECT_URL=https://auth.example.com/social/github/callback || true

# --- a setting given as `<KEY>_FILE` is that setting --------------------------------
github_redirect=https://auth.example.com/social/github/callback
if renders "a client id read from a file in auth's config derives its redirect" "${install[@]}" \
  --set apps.auth.config.SOCIAL__GITHUB__CLIENT_ID_FILE=/vault/github-id --set secrets.SOCIAL__GITHUB__CLIENT_SECRET=stand-in; then
  [ "$(configmap SOCIAL__GITHUB__REDIRECT_URL)" = "$github_redirect" ] || fail "no redirect derived for github"
fi
if renders "a client id read from a file through extraEnv derives its redirect" "${install[@]}" \
  --set secrets.SOCIAL__GITHUB__CLIENT_SECRET=stand-in \
  --set apps.auth.extraEnv[0].name=NESTRS_SOCIAL__GITHUB__CLIENT_ID_FILE --set apps.auth.extraEnv[0].value=/vault/github-id; then
  [ "$(configmap SOCIAL__GITHUB__REDIRECT_URL)" = "$github_redirect" ] || fail "no redirect derived for github"
fi
if renders "a client id read from a file in config derives its redirect" "${install[@]}" \
  --set config.SOCIAL__GOOGLE__CLIENT_ID_FILE=/vault/google-id --set secrets.SOCIAL__GOOGLE__CLIENT_SECRET=stand-in; then
  [ "$(configmap SOCIAL__GOOGLE__REDIRECT_URL)" = https://auth.example.com/social/google/callback ] \
    || fail "no redirect derived for google"
fi
# An app refuses a setting set both as `<KEY>` and as `<KEY>_FILE`, so a pin as a file
# replaces what the chart derives, as an inline pin does.
if renders "an issuer pinned as a file replaces the derived one" "${install[@]}" --set config.AUTHN__ISSUER_FILE=/vault/issuer; then
  [ -z "$(configmap AUTHN__ISSUER)" ] || fail "the derived issuer stands beside the file pinning it"
fi
if renders "an audience pinned as a file replaces each app's own" "${install[@]}" --set config.AUTHN__AUDIENCE_FILE=/vault/audience; then
  [ -z "$(podenv demo-api AUTHN__AUDIENCE)" ] || fail "api's derived audience stands beside the file pinning it"
fi
refuses "a setting handed under both spellings refuses" "- SEAORM__URL, handed to apps\.api both as" \
  "${install[@]}" --set apps.api.config.SEAORM__URL_FILE=/vault/database-url
refuses "a derived setting handed to one app as a file refuses" "- OAUTH__RESOURCE__RESOURCE, handed to apps\.assistant both as" \
  "${install[@]}" --set apps.assistant.config.OAUTH__RESOURCE__RESOURCE_FILE=/vault/resource

if [ "$failures" -gt 0 ]; then
  echo "$failures case(s) failed"
  exit 1
fi
echo "every case holds"
