-- Analytical session 2: Product performance and inventory analytics
-- Runs ~10 SELECT queries focusing on product trends and stock analysis

SELECT p.category, COUNT(*) as product_count, AVG(p.price) as avg_price,
       SUM(p.stock) as total_stock
FROM products p
GROUP BY p.category
ORDER BY total_stock ASC;

SELECT p.name, p.category, p.price, p.stock,
       COUNT(r.id) as review_count, COALESCE(AVG(r.rating), 0) as avg_rating
FROM products p
LEFT JOIN reviews r ON p.id = r.product_id
GROUP BY p.id, p.name, p.category, p.price, p.stock
ORDER BY review_count DESC
LIMIT 30;

SELECT p.name,
       SUM(oi.qty) as total_units_sold,
       SUM(oi.qty * oi.price) as total_revenue,
       COUNT(DISTINCT oi.order_id) as order_appearances
FROM products p
JOIN order_items oi ON p.id = oi.product_id
JOIN orders o ON oi.order_id = o.id
WHERE o.status IN ('shipped', 'delivered')
GROUP BY p.id, p.name
ORDER BY total_revenue DESC
LIMIT 25;

SELECT r.rating, COUNT(*) as count, AVG(length(r.body)) as avg_review_length
FROM reviews r
GROUP BY r.rating
ORDER BY r.rating DESC;

SELECT p.category,
       COUNT(*) FILTER (WHERE p.stock = 0) as out_of_stock,
       COUNT(*) FILTER (WHERE p.stock < 10 AND p.stock > 0) as low_stock,
       COUNT(*) FILTER (WHERE p.stock >= 10) as in_stock
FROM products p
GROUP BY p.category
ORDER BY out_of_stock DESC;

SELECT date_trunc('month', r.created_at) as month,
       COUNT(*) as review_count,
       AVG(r.rating) as avg_rating
FROM reviews r
GROUP BY month
ORDER BY month DESC
LIMIT 12;

SELECT p.name, p.price,
       COUNT(DISTINCT oi.order_id) as times_ordered,
       SUM(oi.qty) as qty_sold
FROM products p
JOIN order_items oi ON p.id = oi.product_id
JOIN orders o ON oi.order_id = o.id
WHERE o.created_at > now() - interval '7 days'
GROUP BY p.id, p.name, p.price
ORDER BY qty_sold DESC
LIMIT 15;

SELECT p1.category, p2.name as frequently_bought_with,
       COUNT(*) as co_occurrence_count
FROM order_items oi1
JOIN order_items oi2 ON oi1.order_id = oi2.order_id AND oi1.product_id <> oi2.product_id
JOIN products p1 ON oi1.product_id = p1.id
JOIN products p2 ON oi2.product_id = p2.id
WHERE p1.category = 'Electronics'
GROUP BY p1.category, p2.name
ORDER BY co_occurrence_count DESC
LIMIT 20;

SELECT c.name as customer, p.name as product, r.rating, r.body, r.created_at
FROM reviews r
JOIN customers c ON r.customer_id = c.id
JOIN products p ON r.product_id = p.id
WHERE r.rating <= 2
ORDER BY r.created_at DESC
LIMIT 20;

SELECT p.name, p.stock,
       CASE
           WHEN p.stock = 0 THEN 'Out of Stock'
           WHEN p.stock < 5 THEN 'Critical'
           WHEN p.stock < 20 THEN 'Low'
           ELSE 'Adequate'
       END as stock_status,
       COALESCE(
           (SELECT SUM(oi.qty) FROM order_items oi
            JOIN orders o ON oi.order_id = o.id
            WHERE oi.product_id = p.id
              AND o.created_at > now() - interval '30 days'), 0
       ) as monthly_velocity
FROM products p
ORDER BY monthly_velocity DESC, p.stock ASC
LIMIT 30;
