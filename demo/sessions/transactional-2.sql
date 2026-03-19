-- Transactional session 2: Place orders (customer IDs 51-120 range)

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (53, 299.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 5, 1, 199.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 16, 2, 49.99);
UPDATE products SET stock = stock - 1 WHERE id = 5 AND stock > 0;
UPDATE products SET stock = stock - 2 WHERE id = 16 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (58, 89.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 23, 3, 29.99);
UPDATE products SET stock = stock - 3 WHERE id = 23 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (62, 179.98, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 8, 2, 89.99);
UPDATE products SET stock = stock - 2 WHERE id = 8 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (67, 54.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 33, 1, 19.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 34, 1, 34.98);
UPDATE products SET stock = stock - 1 WHERE id = 33 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 34 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (71, 449.95, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 19, 1, 349.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 26, 2, 49.98);
UPDATE products SET stock = stock - 1 WHERE id = 19 AND stock > 0;
UPDATE products SET stock = stock - 2 WHERE id = 26 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (76, 24.99, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 40, 1, 24.99);
UPDATE products SET stock = stock - 1 WHERE id = 40 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (80, 159.96, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 13, 4, 39.99);
UPDATE products SET stock = stock - 4 WHERE id = 13 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (85, 109.99, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 17, 1, 109.99);
UPDATE products SET stock = stock - 1 WHERE id = 17 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (91, 264.95, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 21, 5, 52.99);
UPDATE products SET stock = stock - 5 WHERE id = 21 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (95, 74.98, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 29, 2, 37.49);
UPDATE products SET stock = stock - 2 WHERE id = 29 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (100, 219.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 7, 1, 24.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 35, 1, 64.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 36, 1, 129.99);
UPDATE products SET stock = stock - 1 WHERE id = 7 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 35 AND stock > 0;
UPDATE products SET stock = stock - 1 WHERE id = 36 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (108, 44.99, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 38, 1, 44.99);
UPDATE products SET stock = stock - 1 WHERE id = 38 AND stock > 0;
COMMIT;

BEGIN;
INSERT INTO orders (customer_id, total, status, created_at) VALUES (115, 189.97, 'pending', now());
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 24, 1, 89.99);
INSERT INTO order_items (order_id, product_id, qty, price) VALUES (currval('orders_id_seq'), 27, 2, 49.99);
UPDATE products SET stock = stock - 1 WHERE id = 24 AND stock > 0;
UPDATE products SET stock = stock - 2 WHERE id = 27 AND stock > 0;
COMMIT;
