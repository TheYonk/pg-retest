#!/usr/bin/env python3
"""
Variable-rate load driver for pg-retest proxy testing.
Uses asyncpg connection pool with concurrent workers.

Usage: python3 demo/load-driver.py [port] [workers]

Phases:
  1. Warm-up (2 min, no capture): 200 → 350 QPS
  2. Capture window (5 min): 500 → 650 → 700 → 450 → 300 QPS
  3. Cool-down (2 min, no capture): 200 → 500 QPS

Combined QPS is spread across all workers.
"""

import asyncio
import random
import sys
import time
from datetime import datetime

try:
    import asyncpg
except ImportError:
    print("ERROR: asyncpg required. Install with: pip install asyncpg")
    sys.exit(1)

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 5462
WORKERS = int(sys.argv[2]) if len(sys.argv) > 2 else 20

# QPS schedule: (duration_secs, target_qps, phase_name)
SCHEDULE = [
    # Phase 1: Warm-up (no capture)
    (60, 200, "Warm-up min 1"),
    (60, 350, "Warm-up min 2"),
    # Phase 2: Capture window
    (60, 500, "Capture min 1"),
    (60, 650, "Capture min 2"),
    (60, 700, "Capture min 3"),
    (60, 450, "Capture min 4"),
    (60, 300, "Capture min 5"),
    # Phase 3: Cool-down (no capture)
    (60, 200, "Cool-down min 1"),
    (60, 500, "Cool-down min 2"),
]

# Query mix: (weight, sql_template, name)
QUERIES = [
    (25, "SELECT id, name, email FROM customers WHERE id = $1", "select_customer"),
    (15, "SELECT o.id, o.total, o.status, c.name FROM orders o JOIN customers c ON o.customer_id = c.id WHERE o.customer_id = $1 ORDER BY o.created_at DESC LIMIT 5", "select_orders"),
    (15, "SELECT p.category, COUNT(*) as cnt, AVG(p.price) as avg_price FROM products p JOIN order_items oi ON p.id = oi.product_id WHERE oi.order_id BETWEEN $1 AND $1 + 50 GROUP BY p.category", "agg_category"),
    (15, "INSERT INTO reviews (product_id, customer_id, rating, body) VALUES ($1, $2, $3, 'Load test review')", "insert_review"),
    (12, "UPDATE products SET stock = stock + $1 WHERE id = $2", "update_stock"),
    (8, "DELETE FROM reviews WHERE id = (SELECT id FROM reviews WHERE product_id = $1 ORDER BY created_at DESC LIMIT 1)", "delete_review"),
    (5, "SELECT c.name, COUNT(DISTINCT o.id) as orders, SUM(o.total) as total FROM customers c JOIN orders o ON c.id = o.customer_id WHERE c.id = $1 GROUP BY c.name", "join_agg"),
    (5, "SELECT * FROM products WHERE id IN (SELECT product_id FROM order_items WHERE order_id = $1) LIMIT 5", "subquery"),
]

# Build weighted query selector
WEIGHTED_QUERIES = []
for weight, sql, name in QUERIES:
    WEIGHTED_QUERIES.extend([(sql, name)] * weight)


class Stats:
    def __init__(self):
        self.queries = 0
        self.errors = 0
        self.by_type = {}
        self.lock = asyncio.Lock()

    async def record(self, query_type: str, success: bool):
        async with self.lock:
            self.queries += 1
            if not success:
                self.errors += 1
            self.by_type[query_type] = self.by_type.get(query_type, 0) + 1


async def run_query(pool: asyncpg.Pool, stats: Stats):
    """Execute a single random query."""
    sql, name = random.choice(WEIGHTED_QUERIES)

    try:
        async with pool.acquire() as conn:
            if name == "select_customer":
                await conn.fetchrow(sql, random.randint(1, 5000))
            elif name == "select_orders":
                await conn.fetch(sql, random.randint(1, 5000))
            elif name == "agg_category":
                await conn.fetch(sql, random.randint(1, 19950))
            elif name == "insert_review":
                await conn.execute(sql, random.randint(1, 1000), random.randint(1, 5000), random.randint(1, 5))
            elif name == "update_stock":
                await conn.execute(sql, random.randint(1, 10), random.randint(1, 1000))
            elif name == "delete_review":
                await conn.execute(sql, random.randint(1, 1000))
            elif name == "join_agg":
                await conn.fetch(sql, random.randint(1, 5000))
            elif name == "subquery":
                await conn.fetch(sql, random.randint(1, 20000))
        await stats.record(name, True)
    except Exception as e:
        await stats.record(name, False)


async def worker(pool: asyncpg.Pool, stats: Stats, target_qps_per_worker: float, stop_event: asyncio.Event):
    """Worker that runs queries at a target rate."""
    interval = 1.0 / target_qps_per_worker if target_qps_per_worker > 0 else 1.0

    while not stop_event.is_set():
        start = time.monotonic()
        await run_query(pool, stats)
        elapsed = time.monotonic() - start
        sleep_time = max(0, interval - elapsed)
        if sleep_time > 0:
            try:
                await asyncio.wait_for(stop_event.wait(), timeout=sleep_time)
            except asyncio.TimeoutError:
                pass


async def run_phase(pool: asyncpg.Pool, duration: int, target_qps: int, phase_name: str, num_workers: int):
    """Run a phase at a target QPS for a duration."""
    stats = Stats()
    stop_event = asyncio.Event()
    qps_per_worker = target_qps / num_workers

    ts = datetime.now().strftime("%H:%M:%S")
    print(f"  [{ts}] {phase_name}: ~{target_qps} QPS target ({num_workers} workers)", flush=True)

    tasks = [asyncio.create_task(worker(pool, stats, qps_per_worker, stop_event)) for _ in range(num_workers)]

    # Monitor progress every 10 seconds
    start_time = time.monotonic()
    while time.monotonic() - start_time < duration:
        await asyncio.sleep(10)
        elapsed = time.monotonic() - start_time
        actual_qps = stats.queries / elapsed if elapsed > 0 else 0
        ts = datetime.now().strftime("%H:%M:%S")
        print(f"    [{ts}] {stats.queries} queries ({actual_qps:.0f} QPS actual, {stats.errors} errors)", flush=True)

    stop_event.set()
    await asyncio.gather(*tasks, return_exceptions=True)

    elapsed = time.monotonic() - start_time
    actual_qps = stats.queries / elapsed if elapsed > 0 else 0
    ts = datetime.now().strftime("%H:%M:%S")
    print(f"  [{ts}] {phase_name} done: {stats.queries} queries in {elapsed:.0f}s ({actual_qps:.0f} QPS, {stats.errors} errors)")
    print(f"         Mix: {dict(sorted(stats.by_type.items()))}")
    return stats.queries, stats.errors


async def main():
    print(f"=== Variable-Rate Load Driver ===")
    print(f"Target: localhost:{PORT}")
    print(f"Workers: {WORKERS}")
    print(f"Schedule: {len(SCHEDULE)} phases, {sum(d for d, _, _ in SCHEDULE)}s total")
    print()

    pool = await asyncpg.create_pool(
        host="localhost",
        port=PORT,
        user="demo",
        password="demo",
        database="ecommerce",
        min_size=WORKERS,
        max_size=WORKERS + 5,
    )

    total_queries = 0
    total_errors = 0

    for i, (duration, target_qps, phase_name) in enumerate(SCHEDULE):
        if i == 2:  # Before capture phase
            print()
            print(">>> PHASE 2: Start capture now <<<")
            print()
        elif i == 7:  # Before cool-down phase
            print()
            print(">>> PHASE 3: Stop capture now <<<")
            print()

        queries, errors = await run_phase(pool, duration, target_qps, phase_name, WORKERS)
        total_queries += queries
        total_errors += errors

    await pool.close()

    print()
    print(f"=== Complete ===")
    print(f"Total: {total_queries} queries, {total_errors} errors")
    print(f"Duration: {sum(d for d, _, _ in SCHEDULE)}s")


if __name__ == "__main__":
    asyncio.run(main())
