-- Analytical session 1: Revenue and customer analytics
-- Runs ~10 SELECT queries with JOINs and aggregations

SELECT c.name, COUNT(o.id) as order_count, SUM(o.total) as total_spent
FROM customers c JOIN orders o ON c.id = o.customer_id
WHERE o.created_at > now() - interval '30 days'
GROUP BY c.id, c.name ORDER BY total_spent DESC LIMIT 20;

SELECT p.category, COUNT(DISTINCT oi.order_id) as orders, SUM(oi.qty * oi.price) as revenue
FROM products p JOIN order_items oi ON p.id = oi.product_id
JOIN orders o ON oi.order_id = o.id
WHERE o.created_at > now() - interval '90 days'
GROUP BY p.category ORDER BY revenue DESC;

SELECT date_trunc('day', o.created_at) as day, COUNT(*) as orders, SUM(o.total) as revenue
FROM orders o WHERE o.created_at > now() - interval '30 days'
GROUP BY day ORDER BY day;

SELECT p.name, AVG(r.rating) as avg_rating, COUNT(r.id) as review_count
FROM products p JOIN reviews r ON p.id = r.product_id
GROUP BY p.id, p.name HAVING COUNT(r.id) > 3 ORDER BY avg_rating DESC LIMIT 25;

SELECT o.status, COUNT(*) as cnt, AVG(o.total) as avg_total
FROM orders o GROUP BY o.status;

SELECT p.category, p.name, p.stock, p.price
FROM products p WHERE p.stock < 20 ORDER BY p.stock ASC;

SELECT c.id, c.name, c.email, MAX(o.created_at) as last_order
FROM customers c LEFT JOIN orders o ON c.id = o.customer_id
GROUP BY c.id, c.name, c.email
HAVING MAX(o.created_at) < now() - interval '60 days' OR MAX(o.created_at) IS NULL
LIMIT 50;

SELECT date_trunc('week', o.created_at) as week,
       p.category,
       SUM(oi.qty * oi.price) as category_revenue
FROM orders o
JOIN order_items oi ON o.id = oi.order_id
JOIN products p ON oi.product_id = p.id
WHERE o.created_at > now() - interval '90 days'
GROUP BY week, p.category
ORDER BY week DESC, category_revenue DESC;

SELECT c.name, c.email,
       COUNT(o.id) as lifetime_orders,
       SUM(o.total) as lifetime_value,
       MIN(o.created_at) as first_order,
       MAX(o.created_at) as last_order
FROM customers c
JOIN orders o ON c.id = o.customer_id
GROUP BY c.id, c.name, c.email
ORDER BY lifetime_value DESC
LIMIT 30;

SELECT p.name, p.price, p.stock,
       COALESCE(SUM(oi.qty), 0) as units_sold_30d,
       COALESCE(SUM(oi.qty * oi.price), 0) as revenue_30d
FROM products p
LEFT JOIN order_items oi ON p.id = oi.product_id
LEFT JOIN orders o ON oi.order_id = o.id AND o.created_at > now() - interval '30 days'
GROUP BY p.id, p.name, p.price, p.stock
ORDER BY revenue_30d DESC
LIMIT 20;
