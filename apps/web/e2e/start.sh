#!/bin/sh
# Lance un worker et une API jetables sur une base temporaire pour les tests e2e.
set -e
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
DIR=$(mktemp -d)
export SW_DATABASE_PATH="$DIR/e2e.db"
export SW_BIND=127.0.0.1:18090
export SW_JWT_SECRET=e2e-secret-0123456789-0123456789-abcdef
export SW_WEB_DIR="$ROOT/apps/web/dist"
export SW_ALLOWED_INTERNAL_NETWORKS=127.0.0.0/8
"$ROOT/target/debug/sw-worker" &
WORKER=$!
"$ROOT/target/debug/sw-api" &
API=$!
trap 'kill $WORKER $API 2>/dev/null; rm -rf "$DIR"' EXIT INT TERM
wait $API
