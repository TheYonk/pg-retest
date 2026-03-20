#!/usr/bin/env python3
"""Single-phase threaded load driver using psycopg (libpq).
Usage: python3 load-driver-phase.py PORT DURATION_SECS TARGET_QPS WORKERS
"""
import random, sys, time, threading
from datetime import datetime
from psycopg.rows import dict_row
import psycopg

PORT = int(sys.argv[1])
DURATION = int(sys.argv[2])
TARGET_QPS = int(sys.argv[3])
WORKERS = int(sys.argv[4]) if len(sys.argv) > 4 else 20
CONNSTR = f"host=localhost port={PORT} dbname=ecommerce user=demo password=demo connect_timeout=5"

queries_done = 0
errors = 0
lock = threading.Lock()

def pick_query():
    r = random.randint(0, 99)
    cid = random.randint(1, 5000)
    pid = random.randint(1, 1000)
    oid = random.randint(1, 20000)
    if r < 25:
        return f"SELECT id, name, email FROM customers WHERE id = {cid}"
    elif r < 40:
        return f"SELECT o.id, o.total, o.status FROM orders o WHERE o.customer_id = {cid} ORDER BY o.created_at DESC LIMIT 5"
    elif r < 55:
        cats = ['Electronics','Clothing','Books','Home','Sports','Toys','Food','Beauty','Garden','Auto']
        cat = random.choice(cats)
        return f"SELECT p.category, COUNT(*), AVG(p.price) FROM products p JOIN order_items oi ON p.id = oi.product_id JOIN orders o ON oi.order_id = o.id WHERE p.category = '{cat}' AND o.created_at > now() - interval '30 days' GROUP BY p.category"
    elif r < 70:
        return f"INSERT INTO reviews (product_id, customer_id, rating, body) VALUES ({pid}, {cid}, {random.randint(1,5)}, 'load test {random.randint(1,99999)}')"
    elif r < 82:
        return f"UPDATE products SET stock = stock + {random.randint(1,10)} WHERE id = {pid}"
    elif r < 90:
        return f"DELETE FROM reviews WHERE id = (SELECT id FROM reviews WHERE product_id = {pid} ORDER BY created_at DESC LIMIT 1)"
    elif r < 95:
        return f"SELECT c.name, COUNT(DISTINCT o.id), SUM(o.total) FROM customers c JOIN orders o ON c.id = o.customer_id WHERE c.id = {cid} GROUP BY c.name"
    else:
        return f"SELECT * FROM products WHERE id IN (SELECT product_id FROM order_items WHERE order_id = {oid}) LIMIT 5"

def worker(worker_id, interval, stop_event):
    global queries_done, errors
    conn = psycopg.connect(CONNSTR, autocommit=True)
    while not stop_event.is_set():
        try:
            sql = pick_query()
            conn.execute(sql)
            with lock:
                queries_done += 1
        except Exception as e:
            with lock:
                errors += 1
                queries_done += 1
            # Reconnect on error
            try:
                conn.close()
            except:
                pass
            try:
                conn = psycopg.connect(CONNSTR, autocommit=True)
            except:
                time.sleep(0.5)
        time.sleep(max(0, interval + random.uniform(-interval*0.3, interval*0.3)))
    conn.close()

def main():
    global queries_done, errors
    interval = 1.0 / (TARGET_QPS / WORKERS)
    stop = threading.Event()
    ts = datetime.now().strftime("%H:%M:%S")
    print(f"  [{ts}] {TARGET_QPS} QPS target, {WORKERS} workers, {DURATION}s", flush=True)

    threads = []
    for i in range(WORKERS):
        t = threading.Thread(target=worker, args=(i, interval, stop), daemon=True)
        t.start()
        threads.append(t)

    start = time.monotonic()
    while time.monotonic() - start < DURATION:
        time.sleep(10)
        elapsed = time.monotonic() - start
        with lock:
            qps = queries_done / elapsed if elapsed > 0 else 0
            ts = datetime.now().strftime("%H:%M:%S")
            print(f"    [{ts}] {queries_done} queries ({qps:.0f} QPS, {errors} errs)", flush=True)

    stop.set()
    for t in threads:
        t.join(timeout=5)
    elapsed = time.monotonic() - start
    ts = datetime.now().strftime("%H:%M:%S")
    with lock:
        print(f"  [{ts}] Done: {queries_done} queries in {elapsed:.0f}s ({queries_done/elapsed:.0f} QPS, {errors} errs)", flush=True)

if __name__ == "__main__":
    main()
