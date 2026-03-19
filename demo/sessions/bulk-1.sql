-- Bulk session 1: Batch order status progression and inventory reconciliation
-- Simulates a background job running order fulfillment pipeline in chunks

-- Chunk 0: advance pending -> shipped and shipped -> delivered
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 0;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 0;

SELECT COUNT(*) as chunk0_shipped FROM orders WHERE status = 'shipped' AND id % 10 = 0;

SELECT pg_sleep(2);

-- Chunk 1
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 1;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 1;

SELECT pg_sleep(2);

-- Chunk 2
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 2;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 2;

SELECT pg_sleep(2);

-- Chunk 3
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 3;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 3;

SELECT pg_sleep(2);

-- Chunk 4
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 4;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 4;

-- Inventory reconciliation: flag products with zero stock
SELECT p.id, p.name, p.category, p.stock
FROM products p
WHERE p.stock = 0
ORDER BY p.category, p.name;

SELECT pg_sleep(2);

-- Chunk 5
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 5;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 5;

SELECT pg_sleep(2);

-- Chunk 6
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 6;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 6;

SELECT pg_sleep(2);

-- Chunk 7
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 7;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 7;

-- Batch summary report
SELECT o.status, COUNT(*) as count, SUM(o.total) as total_value
FROM orders o
GROUP BY o.status
ORDER BY count DESC;

SELECT pg_sleep(2);

-- Chunk 8
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 8;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 8;

-- Chunk 9
UPDATE orders SET status = 'shipped'
WHERE status = 'pending'
  AND created_at < now() - interval '7 days'
  AND id % 10 = 9;

UPDATE orders SET status = 'delivered'
WHERE status = 'shipped'
  AND created_at < now() - interval '14 days'
  AND id % 10 = 9;

-- Final inventory summary after bulk processing
SELECT p.category, COUNT(*) as product_count,
       SUM(CASE WHEN p.stock = 0 THEN 1 ELSE 0 END) as out_of_stock,
       AVG(p.stock) as avg_stock
FROM products p
GROUP BY p.category
ORDER BY out_of_stock DESC;
