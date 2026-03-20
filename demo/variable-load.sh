#!/usr/bin/env bash
# Variable-rate workload with mixed query types (INSERT, UPDATE, DELETE, SELECT)
# Varies QPS minute-by-minute between 200-700 QPS
# Usage: bash demo/variable-load.sh [port]
set -euo pipefail

PORT=${1:-5462}
export PGPASSWORD=demo

# Per-minute target QPS schedule
QPS_SCHEDULE=(200 350 500 650 700 450 300 200 500)

run_mixed_queries() {
    local duration=$1
    local target_qps=$2
    local end_time=$((SECONDS + duration))
    local count=0
    local sleep_us=$((1000000 / target_qps))

    while [ $SECONDS -lt $end_time ]; do
        local r=$((RANDOM % 100))
        local cid=$((RANDOM % 5000 + 1))
        local pid=$((RANDOM % 1000 + 1))
        local oid=$((RANDOM % 20000 + 1))

        if [ $r -lt 25 ]; then
            # 25% - Simple SELECT
            psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                "SELECT id, name, email FROM customers WHERE id = $cid;" > /dev/null 2>&1
        elif [ $r -lt 40 ]; then
            # 15% - JOIN SELECT
            psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                "SELECT o.id, o.total, o.status, c.name FROM orders o JOIN customers c ON o.customer_id = c.id WHERE o.id = $oid;" > /dev/null 2>&1
        elif [ $r -lt 55 ]; then
            # 15% - Aggregation
            psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                "SELECT p.category, COUNT(*) as cnt, SUM(oi.qty * oi.price) as revenue FROM products p JOIN order_items oi ON p.id = oi.product_id WHERE p.category = (ARRAY['Electronics','Clothing','Books','Home','Sports','Toys','Food','Beauty','Garden','Auto'])[$((RANDOM % 10 + 1))] GROUP BY p.category;" > /dev/null 2>&1
        elif [ $r -lt 70 ]; then
            # 15% - INSERT (transactional)
            psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                "INSERT INTO reviews (product_id, customer_id, rating, body) VALUES ($pid, $cid, $((RANDOM % 5 + 1)), 'Load test review #$count');" > /dev/null 2>&1
        elif [ $r -lt 82 ]; then
            # 12% - UPDATE
            psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                "UPDATE products SET stock = stock + $((RANDOM % 10 + 1)) WHERE id = $pid;" > /dev/null 2>&1
        elif [ $r -lt 90 ]; then
            # 8% - DELETE + re-INSERT (simulate churn)
            psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                "DELETE FROM reviews WHERE id = (SELECT id FROM reviews WHERE product_id = $pid ORDER BY created_at DESC LIMIT 1);" > /dev/null 2>&1
        elif [ $r -lt 95 ]; then
            # 5% - Multi-table JOIN
            psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                "SELECT c.name, COUNT(DISTINCT o.id), SUM(o.total) FROM customers c JOIN orders o ON c.id = o.customer_id JOIN order_items oi ON o.id = oi.order_id WHERE c.id = $cid GROUP BY c.name;" > /dev/null 2>&1
        else
            # 5% - Subquery
            psql -h localhost -p $PORT -U demo -d ecommerce -t -c \
                "SELECT * FROM products WHERE id IN (SELECT product_id FROM order_items WHERE order_id = $oid) LIMIT 5;" > /dev/null 2>&1
        fi
        count=$((count + 1))
    done
    echo $count
}

echo "=== Variable-Rate Load Test ==="
echo "Port: $PORT"
echo ""

# Run workers — each handles ~25 QPS
run_at_rate() {
    local duration=$1
    local target_qps=$2
    local workers=$((target_qps / 25))
    [ $workers -lt 1 ] && workers=1
    local actual_per_worker=$((target_qps / workers))

    echo "[$(date +%H:%M:%S)] Running at ~${target_qps} QPS (${workers} workers) for ${duration}s"

    local pids=()
    for i in $(seq 1 $workers); do
        run_mixed_queries $duration $actual_per_worker &
        pids+=($!)
    done

    local total=0
    for pid in "${pids[@]}"; do
        wait $pid
        result=$?
    done
    echo "[$(date +%H:%M:%S)] Phase complete"
}

# Phase 1: 2 minutes with NO capture (proxy running, capture off)
echo ""
echo "=== Phase 1: Warm-up (NO capture) — 2 minutes ==="
echo "  Minute 1: 200 QPS"
run_at_rate 60 200
echo "  Minute 2: 350 QPS"
run_at_rate 60 350

echo ""
echo ">>> SIGNAL: Ready for capture start <<<"
echo ""

# Phase 2: 5 minutes WITH capture
echo "=== Phase 2: Capture ON — 5 minutes ==="
echo "  Minute 3: 500 QPS"
run_at_rate 60 500
echo "  Minute 4: 650 QPS"
run_at_rate 60 650
echo "  Minute 5: 700 QPS"
run_at_rate 60 700
echo "  Minute 6: 450 QPS"
run_at_rate 60 450
echo "  Minute 7: 300 QPS"
run_at_rate 60 300

echo ""
echo ">>> SIGNAL: Ready for capture stop <<<"
echo ""

# Phase 3: 2 minutes with NO capture (proxy still running)
echo "=== Phase 3: Cool-down (NO capture) — 2 minutes ==="
echo "  Minute 8: 200 QPS"
run_at_rate 60 200
echo "  Minute 9: 500 QPS"
run_at_rate 60 500

echo ""
echo "=== Load test complete ==="
