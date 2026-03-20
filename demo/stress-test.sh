#!/usr/bin/env bash
# Stress test: ~500 QPS for a configurable duration against a PG target
# Usage: bash demo/stress-test.sh [duration_seconds] [target_qps] [port]
set -euo pipefail

DURATION=${1:-600}  # default 10 minutes
TARGET_QPS=${2:-500}
PORT=${3:-5462}
CONNSTR="host=localhost port=$PORT dbname=ecommerce user=demo"

# We'll run N parallel workers, each doing queries in a tight loop with sleep
# Each worker does ~25 QPS, so 20 workers = ~500 QPS
WORKERS=$((TARGET_QPS / 25))
QUERIES_PER_WORKER=$((TARGET_QPS * DURATION / WORKERS))
SLEEP_MS=40  # ~25 queries/sec per worker

echo "Stress test: ${WORKERS} workers, ~${TARGET_QPS} QPS target, ${DURATION}s duration"
echo "Target: localhost:${PORT}"
echo "Estimated total queries: $((TARGET_QPS * DURATION))"
echo ""

# Mix of query types
run_worker() {
    local worker_id=$1
    local count=0
    local end_time=$((SECONDS + DURATION))

    while [ $SECONDS -lt $end_time ]; do
        # Rotate through different query types
        case $((count % 10)) in
            0|1|2) # 30% - simple lookups
                PGPASSWORD=demo psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                    "SELECT id, name, email FROM customers WHERE id = $((RANDOM % 5000 + 1));" > /dev/null 2>&1
                ;;
            3|4) # 20% - order lookups
                PGPASSWORD=demo psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                    "SELECT o.id, o.total, o.status FROM orders o WHERE o.customer_id = $((RANDOM % 5000 + 1)) ORDER BY o.created_at DESC LIMIT 5;" > /dev/null 2>&1
                ;;
            5|6) # 20% - analytical joins
                PGPASSWORD=demo psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                    "SELECT p.category, COUNT(*) as cnt, AVG(p.price) as avg_price FROM products p JOIN order_items oi ON p.id = oi.product_id WHERE oi.order_id BETWEEN $((RANDOM % 19000 + 1)) AND $((RANDOM % 19000 + 100)) GROUP BY p.category;" > /dev/null 2>&1
                ;;
            7) # 10% - insert (transaction)
                PGPASSWORD=demo psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                    "BEGIN; INSERT INTO reviews (product_id, customer_id, rating, body) VALUES ($((RANDOM % 1000 + 1)), $((RANDOM % 5000 + 1)), $((RANDOM % 5 + 1)), 'stress test review'); COMMIT;" > /dev/null 2>&1
                ;;
            8) # 10% - update
                PGPASSWORD=demo psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                    "UPDATE products SET stock = stock + 1 WHERE id = $((RANDOM % 1000 + 1));" > /dev/null 2>&1
                ;;
            9) # 10% - aggregation
                PGPASSWORD=demo psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                    "SELECT status, COUNT(*), AVG(total) FROM orders WHERE created_at > now() - interval '$((RANDOM % 30 + 1)) days' GROUP BY status;" > /dev/null 2>&1
                ;;
        esac
        count=$((count + 1))

        # Tiny sleep to pace
        sleep 0.0${SLEEP_MS}
    done
    echo "Worker $worker_id: $count queries"
}

# Launch all workers
for i in $(seq 1 $WORKERS); do
    run_worker $i &
done

echo "All $WORKERS workers launched. Running for ${DURATION}s..."
echo "Check status: ./target/release/pg-retest proxy-ctl --proxy localhost:9091 status"
echo ""

# Wait for all
wait
echo ""
echo "Stress test complete."
