-- Mixed session 2: Customer account reads + cart/wishlist write patterns
-- Simulates browsing products, viewing order history, updating account

SELECT id, name, email, created_at FROM customers WHERE id = 150;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 150 ORDER BY o.created_at DESC LIMIT 5;
SELECT COUNT(*) as total_orders, SUM(o.total) as lifetime_value FROM orders o WHERE o.customer_id = 150;

SELECT p.id, p.name, p.price, p.stock, p.category FROM products p WHERE p.category = 'Electronics' ORDER BY p.price ASC LIMIT 10;
SELECT p.id, p.name, p.price, AVG(r.rating) as avg_rating
FROM products p LEFT JOIN reviews r ON p.id = r.product_id
WHERE p.category = 'Electronics'
GROUP BY p.id, p.name, p.price ORDER BY avg_rating DESC LIMIT 5;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (9, 150, 4, 'Works well, good build quality', now());
COMMIT;

SELECT id, name, email, created_at FROM customers WHERE id = 250;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 250 ORDER BY o.created_at DESC LIMIT 10;
SELECT oi.product_id, p.name, oi.qty, oi.price FROM order_items oi JOIN products p ON oi.product_id = p.id WHERE oi.order_id IN (SELECT id FROM orders WHERE customer_id = 250 ORDER BY created_at DESC LIMIT 1);

SELECT p.id, p.name, p.price, p.stock FROM products p WHERE p.category = 'Clothing' ORDER BY p.stock DESC LIMIT 15;
SELECT AVG(rating) as avg_rating, COUNT(*) as review_count FROM reviews WHERE product_id = 41;
SELECT r.rating, r.body, c.name FROM reviews r JOIN customers c ON r.customer_id = c.id WHERE r.product_id = 41 ORDER BY r.created_at DESC LIMIT 3;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (41, 250, 5, 'Amazing quality, fits perfectly', now());
COMMIT;

SELECT id, name, email, created_at FROM customers WHERE id = 350;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 350 ORDER BY o.created_at DESC LIMIT 10;

SELECT p.id, p.name, p.price, p.stock FROM products p WHERE p.id IN (12, 19, 23, 35, 44);
SELECT r.product_id, AVG(r.rating) as avg_rating, COUNT(*) as cnt FROM reviews r WHERE r.product_id IN (12, 19, 23, 35, 44) GROUP BY r.product_id;

BEGIN;
UPDATE customers SET email = 'updated350@example.com' WHERE id = 350;
COMMIT;

SELECT id, name, email FROM customers WHERE id = 350;

SELECT id, name, email, created_at FROM customers WHERE id = 450;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 450 ORDER BY o.created_at DESC LIMIT 5;

SELECT p.id, p.name, p.price, p.stock FROM products p WHERE p.price BETWEEN 20.00 AND 80.00 ORDER BY p.price ASC LIMIT 20;
SELECT p.id, p.name, p.price FROM products p WHERE p.stock > 50 AND p.category = 'Books' ORDER BY p.price;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (36, 450, 3, 'Average product, nothing special', now());
COMMIT;

SELECT id, name, email FROM customers WHERE id = 550;
SELECT o.id, o.total, o.status, o.created_at FROM orders o WHERE o.customer_id = 550 ORDER BY o.created_at DESC LIMIT 8;
SELECT COUNT(DISTINCT o.id) as order_count, SUM(o.total) as total_spent FROM orders o WHERE o.customer_id = 550;

BEGIN;
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
VALUES (18, 550, 4, 'Good value for the price', now());
COMMIT;

SELECT AVG(rating) as new_avg, COUNT(*) as total_reviews FROM reviews WHERE product_id = 18;
