-- Mixed session 1: Customer self-service reads + light writes
-- Simulates customers browsing their accounts and leaving reviews

SELECT id, name, email, created_at FROM customers WHERE id = 100;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 100 ORDER BY o.created_at DESC LIMIT 10;
SELECT oi.product_id, p.name, oi.qty, oi.price FROM order_items oi JOIN products p ON oi.product_id = p.id WHERE oi.order_id IN (SELECT id FROM orders WHERE customer_id = 100 LIMIT 3);

SELECT p.id, p.name, p.price, p.stock, p.category FROM products p WHERE p.id = 50;
SELECT AVG(rating) as avg_rating, COUNT(*) as review_count FROM reviews WHERE product_id = 50;
SELECT r.rating, r.body, c.name, r.created_at FROM reviews r JOIN customers c ON r.customer_id = c.id WHERE r.product_id = 50 ORDER BY r.created_at DESC LIMIT 5;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (50, 100, 4, 'Great product, fast shipping', now());
COMMIT;

SELECT p.id, p.name, p.price, p.stock FROM products p WHERE p.id = 22;
SELECT AVG(rating) as avg_rating FROM reviews WHERE product_id = 22;

SELECT id, name, email, created_at FROM customers WHERE id = 200;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 200 ORDER BY o.created_at DESC LIMIT 10;
SELECT oi.product_id, p.name, oi.qty, oi.price FROM order_items oi JOIN products p ON oi.product_id = p.id WHERE oi.order_id IN (SELECT id FROM orders WHERE customer_id = 200 LIMIT 3);

SELECT p.id, p.name, p.price, p.stock, p.category FROM products p WHERE p.id = 33;
SELECT AVG(rating) as avg_rating, COUNT(*) as review_count FROM reviews WHERE product_id = 33;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (33, 200, 5, 'Exceeded expectations, highly recommend', now());
COMMIT;

SELECT id, name, email, created_at FROM customers WHERE id = 300;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 300 ORDER BY o.created_at DESC LIMIT 10;

SELECT p.id, p.name, p.price, p.stock FROM products p WHERE p.id = 15;
SELECT r.rating, r.body, r.created_at FROM reviews r WHERE r.product_id = 15 ORDER BY r.created_at DESC LIMIT 3;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (15, 300, 3, 'Decent quality but packaging was damaged', now());
COMMIT;

SELECT id, name, email, created_at FROM customers WHERE id = 400;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 400 ORDER BY o.created_at DESC LIMIT 5;

SELECT p.id, p.name, p.price, p.category FROM products p WHERE p.id = 7;
SELECT AVG(rating) as avg_rating, COUNT(*) as review_count FROM reviews WHERE product_id = 7;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (7, 400, 5, 'Perfect, exactly as described', now());
COMMIT;

SELECT id, name, email, created_at FROM customers WHERE id = 500;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 500 ORDER BY o.created_at DESC LIMIT 10;
SELECT oi.product_id, p.name, oi.qty, oi.price FROM order_items oi JOIN products p ON oi.product_id = p.id WHERE oi.order_id IN (SELECT id FROM orders WHERE customer_id = 500 LIMIT 2);

SELECT p.id, p.name, p.price, p.stock FROM products p WHERE p.id = 28;
SELECT AVG(rating) as avg_rating FROM reviews WHERE product_id = 28;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (28, 500, 2, 'Not what I expected, poor value', now());
COMMIT;
