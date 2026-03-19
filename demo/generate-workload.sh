#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

CONTAINER_NAME="pg-retest-demo-gen"
DB_PORT="${DB_PORT:-5460}"
PROXY_PORT="${PROXY_PORT:-5461}"
OUTPUT="${OUTPUT:-$SCRIPT_DIR/workload.wkl}"

cleanup() {
    echo "==> Cleaning up..."
    docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> Starting temporary PostgreSQL container..."
docker run -d --name "$CONTAINER_NAME" \
    -e POSTGRES_DB=ecommerce \
    -e POSTGRES_USER=demo \
    -e POSTGRES_PASSWORD=demo \
    -p "${DB_PORT}:5432" \
    -v "$SCRIPT_DIR/init-db-a.sql:/docker-entrypoint-initdb.d/init.sql" \
    postgres:16

echo "==> Waiting for PostgreSQL to be ready..."
until docker exec "$CONTAINER_NAME" pg_isready -U demo -d ecommerce 2>/dev/null; do
    sleep 1
done
sleep 2

echo "==> Building pg-retest..."
(cd "$PROJECT_DIR" && cargo build --release 2>&1)
PGRETEST="$PROJECT_DIR/target/release/pg-retest"

echo "==> Starting proxy capture on port $PROXY_PORT -> localhost:$DB_PORT..."
"$PGRETEST" proxy \
    --listen "0.0.0.0:${PROXY_PORT}" \
    --target "localhost:${DB_PORT}" \
    --output "$OUTPUT" &
PROXY_PID=$!

# Give the proxy a moment to bind and begin listening
sleep 2

echo "==> Running workload sessions in parallel..."
SESSION_PIDS=()
for session_file in "$SCRIPT_DIR"/sessions/*.sql; do
    echo "    Starting session: $(basename "$session_file")"
    PGPASSWORD=demo psql \
        -h localhost \
        -p "$PROXY_PORT" \
        -U demo \
        -d ecommerce \
        -f "$session_file" \
        > /dev/null 2>&1 &
    SESSION_PIDS+=($!)
done

echo "==> Waiting for all sessions to complete..."
for pid in "${SESSION_PIDS[@]}"; do
    wait "$pid" || true   # ignore individual session errors (e.g. no rows matched)
done

echo "==> All sessions finished. Stopping proxy..."
kill "$PROXY_PID" 2>/dev/null || true
wait "$PROXY_PID" 2>/dev/null || true

echo ""
echo "==> Done! Demo workload saved to: $OUTPUT"
echo ""
echo "==> Workload summary:"
"$PGRETEST" inspect "$OUTPUT" --classify
