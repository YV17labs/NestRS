#!/usr/bin/env bash
# The Publish "magic moment": one JWT drives row-level scoping and field
# masking with no authorization code in any handler. Run it after
# `nestrs run db reset`, with the `auth` (3001) and `api` (3002) apps up.
#
#   nestrs run demo        # (or) bash scripts/magic-moment.sh
#
# Every seeded account shares the password `publish-demo`.
set -euo pipefail

AUTH=${AUTH_URL:-http://localhost:3001}
API=${API_URL:-http://localhost:3002}
PASSWORD=${DEMO_PASSWORD:-publish-demo}
# A Globex user id — cross-tenant for an Acme member (seed: GLOBEX_AUTHOR).
GLOBEX_USER=00000000-0000-7000-8000-000000006101

need() { command -v "$1" >/dev/null || { echo "error: '$1' is required" >&2; exit 1; }; }
need curl
need jq

login() {
  curl -sf "$AUTH/login" -H 'content-type: application/json' \
    -d "{\"email\":\"$1\",\"password\":\"$PASSWORD\"}" | jq -r .access_token
}

hr() { printf '\n\033[1m%s\033[0m\n' "$1"; }

if ! curl -sf -o /dev/null "$API/health/live" 2>/dev/null; then
  echo "error: the api app is not reachable at $API — run 'nestrs run dev api' first" >&2
  exit 1
fi

hr "1. Sign in as the Acme admin"
ADMIN=$(login admin@acme.test)
echo "   token acquired"

hr "2. Admin GET /users — every field, Acme rows only"
curl -sf "$API/users" -H "authorization: Bearer $ADMIN" | jq -c '.[]'

hr "3. Sign in as a plain Acme member, run the identical query"
MEMBER=$(login acme-user-1@example.test)
curl -sf "$API/users" -H "authorization: Bearer $MEMBER" | jq -c '.[]'
echo "   → same rows, but 'email' is gone from every object"

hr "4. Member reads a Globex user by id — refused across the tenant boundary"
code=$(curl -s -o /dev/null -w '%{http_code}' \
  "$API/users/$GLOBEX_USER" -H "authorization: Bearer $MEMBER")
echo "   HTTP $code (expected 403)"
[ "$code" = "403" ] || { echo "error: expected 403, got $code" >&2; exit 1; }

hr "The handler never read the tenant or the role — the framework carried it."
