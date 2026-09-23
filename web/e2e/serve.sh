#!/usr/bin/env bash
# Runs one API instance for an end-to-end run.
#
#   e2e/serve.sh <port> <allowed-subject> <database>
#
# Two instances are started, differing only in the allow-list, because that is
# the one piece of auth behaviour a single configuration cannot show both sides
# of: the stub issues one fixed subject, so a server that admits it and a
# server that refuses it have to be different servers.
#
# Each gets its own database and its own directories. Sharing a database would
# put two job-worker pools on one queue, and ADR-0003 assumes exactly one.
set -euo pipefail

PORT="${1:?port}"
SUBJECT="${2:?allowed subject}"
DB_NAME="${3:?database name}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ADMIN_URL="${DATABASE_URL:?set DATABASE_URL to a PostgreSQL this may create databases on}"

# A run starts from an empty database on purpose: a test that depends on what
# a previous run left behind passes until someone runs it alone.
psql "$ADMIN_URL" -q -c "DROP DATABASE IF EXISTS $DB_NAME WITH (FORCE)" \
                  -c "CREATE DATABASE $DB_NAME" >/dev/null

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/library" "$WORK/data"

# The frontend the API serves is the built one (ADR-0006), so a run exercises
# the real static-file routes and the real same-origin cookie — not the dev
# server's proxy.
if [ ! -f "$ROOT/web/dist/index.html" ]; then
  echo "e2e: web/dist is missing — run 'npm run build' in web/ first" >&2
  exit 1
fi

exec env \
  DATABASE_URL="${ADMIN_URL%/*}/$DB_NAME" \
  LIBRARY_ROOT="$WORK/library" \
  DATA_DIR="$WORK/data" \
  BIND_ADDR="127.0.0.1:$PORT" \
  AUTH_MODE=oidc \
  OIDC_ISSUER_URL="http://127.0.0.1:18091/default" \
  OIDC_CLIENT_ID=nyuka \
  OIDC_CLIENT_SECRET=shh \
  OIDC_REDIRECT_URL="http://127.0.0.1:$PORT/api/v1/auth/callback" \
  AUTH_ALLOWED_SUBJECTS="$SUBJECT" \
  SESSION_KEY="e2e0000000000000000000000000000000000000000000000000000000000001" \
  "$ROOT/target/debug/nyuka-api"
