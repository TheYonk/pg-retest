-- Transactional session 1: Place orders (customer IDs 1-50 range)

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (1, 149.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 3, 1, 99.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 7, 2, 24.99);
UPDATE products SET stock = stock - 1 WHERE id = 3 AND stock > 0;
UPDATE products SET stock = stock - 2 WHERE id = 7 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (5, 74.98, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 12, 2, 37.49);
UPDATE products SET stock = stock - 2 WHERE id = 12 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (8, 219.95, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 2, 1, 149.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 15, 1, 39.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 18, 1, 29.97);
UPDATE products SET stock = stock - 1 WHERE id = 2 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 15 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 18 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (13, 59.99, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 20, 1, 59.99);
UPDATE products SET stock = stock - 1 WHERE id = 20 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (17, 329.94, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 1, 2, 89.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 9, 3, 49.99);
UPDATE products SET stock = stock - 2 WHERE id = 1 AND stock > 0;
UPDATE products SET stock = stock - 3 WHERE id = 9 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (22, 44.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 30, 3, 14.99);
UPDATE products SET stock = stock - 3 WHERE id = 30 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (25, 189.96, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 4, 4, 47.49);
UPDATE products SET stock = stock - 4 WHERE id = 4 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (31, 109.98, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 6, 1, 79.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 22, 1, 29.99);
UPDATE products SET stock = stock - 1 WHERE id = 6 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 22 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (36, 264.96, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 11, 6, 44.16);
UPDATE products SET stock = stock - 6 WHERE id = 11 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (42, 150.00, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 10, 2, 49.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 25, 1, 50.02);
UPDATE products SET stock = stock - 2 WHERE id = 10 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 25 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (47, 39.99, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 28, 1, 39.99);
UPDATE products SET stock = stock - 1 WHERE id = 28 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (50, 134.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 14, 3, 44.99);
UPDATE products SET stock = stock - 3 WHERE id = 14 AND stock > 0;
COMMIT;
