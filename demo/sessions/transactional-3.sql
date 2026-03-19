-- Transactional session 3: Place orders + order status updates (customer IDs 120-200 range)

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (121, 129.98, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 31, 2, 64.99);
UPDATE products SET stock = stock - 2 WHERE id = 31 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (126, 59.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 32, 3, 19.99);
UPDATE products SET stock = stock - 3 WHERE id = 32 AND stock > 0;
COMMIT;

SELECT id, total, status FROM orders WHERE customer_id = 121 ORDER BY created_at DESC LIMIT 5;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (130, 399.96, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 37, 4, 99.99);
UPDATE products SET stock = stock - 4 WHERE id = 37 AND stock > 0;
COMMIT;

BEGIN;
UPDATE orders SET status = 'processing'
WHERE status = 'pending'
  AND customer_id IN (121, 126)
  AND created_at > now() - interval '5 minutes';
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (135, 249.96, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 39, 3, 49.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 41, 1, 99.99);
UPDATE products SET stock = stock - 3 WHERE id = 39 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 41 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (142, 79.98, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 42, 2, 39.99);
UPDATE products SET stock = stock - 2 WHERE id = 42 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (148, 174.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 43, 1, 124.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 44, 1, 49.98);
UPDATE products SET stock = stock - 1 WHERE id = 43 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 44 AND stock > 0;
COMMIT;

BEGIN;
UPDATE orders SET status = 'processing'
WHERE status = 'pending'
  AND customer_id IN (130, 135)
  AND created_at > now() - interval '5 minutes';
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (155, 34.99, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 45, 1, 34.99);
UPDATE products SET stock = stock - 1 WHERE id = 45 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (163, 314.96, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 46, 4, 78.74);
UPDATE products SET stock = stock - 4 WHERE id = 46 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (170, 99.99, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 47, 1, 99.99);
UPDATE products SET stock = stock - 1 WHERE id = 47 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (178, 224.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 48, 3, 74.99);
UPDATE products SET stock = stock - 3 WHERE id = 48 AND stock > 0;
COMMIT;

BEGIN;
UPDATE orders SET status = 'processing'
WHERE status = 'pending'
  AND customer_id IN (142, 148, 155)
  AND created_at > now() - interval '5 minutes';
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (185, 54.98, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 49, 2, 27.49);
UPDATE products SET stock = stock - 2 WHERE id = 49 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (192, 289.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 50, 1, 189.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 3, 2, 49.99);
UPDATE products SET stock = stock - 1 WHERE id = 50 AND stock > 0;
UPDATE products SET stock = stock - 2 WHERE id = 3 AND stock > 0;
COMMIT;
