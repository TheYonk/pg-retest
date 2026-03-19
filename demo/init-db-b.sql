-- E-Commerce Demo Schema + Seed Data for pg-retest

-- Schema
CREATE TABLE customers (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE products (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT NOT NULL,
    price NUMERIC(10,2) NOT NULL,
    stock INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers(id),
    total NUMERIC(10,2) NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE order_items (
    id SERIAL PRIMARY KEY,
    order_id INTEGER NOT NULL REFERENCES orders(id),
    product_id INTEGER NOT NULL REFERENCES products(id),
    qty INTEGER NOT NULL DEFAULT 1,
    price NUMERIC(10,2) NOT NULL
);

CREATE TABLE reviews (
    id SERIAL PRIMARY KEY,
    product_id INTEGER NOT NULL REFERENCES products(id),
    customer_id INTEGER NOT NULL REFERENCES customers(id),
    rating INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
    body TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexes
CREATE INDEX idx_orders_customer_id ON orders(customer_id);
CREATE INDEX idx_orders_status_created ON orders(status, created_at);
CREATE INDEX idx_orders_customer_created ON orders(customer_id, created_at);
CREATE INDEX idx_order_items_order_id ON order_items(order_id);
CREATE INDEX idx_order_items_product_id ON order_items(product_id);
CREATE INDEX idx_order_items_product_qty ON order_items(product_id, qty);
CREATE INDEX idx_reviews_product_id ON reviews(product_id);
CREATE INDEX idx_reviews_product_rating ON reviews(product_id, rating);
CREATE INDEX idx_reviews_customer_id ON reviews(customer_id);

-- Seed data: Customers (~5,000)
INSERT INTO customers (name, email, created_at)
SELECT
    'Customer ' || i,
    'customer' || i || '@example.com',
    now() - (random() * interval '365 days')
FROM generate_series(1, 5000) AS i;

-- Seed data: Products (~1,000)
INSERT INTO products (name, category, price, stock)
SELECT
    'Product ' || i,
    (ARRAY['Electronics', 'Clothing', 'Books', 'Home', 'Sports',
           'Toys', 'Food', 'Beauty', 'Garden', 'Auto'])[1 + (i % 10)],
    round((random() * 200 + 5)::numeric, 2),
    (random() * 500)::integer
FROM generate_series(1, 1000) AS i;

-- Seed data: Orders (~20,000)
INSERT INTO orders (customer_id, total, status, created_at)
SELECT
    1 + (random() * 4999)::integer,
    round((random() * 500 + 10)::numeric, 2),
    (ARRAY['pending', 'shipped', 'delivered', 'cancelled'])[1 + (i % 4)],
    now() - (random() * interval '180 days')
FROM generate_series(1, 20000) AS i;

-- Seed data: Order Items (~60,000, ~3 per order)
INSERT INTO order_items (order_id, product_id, qty, price)
SELECT
    1 + (i / 3),
    1 + (random() * 999)::integer,
    1 + (random() * 4)::integer,
    round((random() * 100 + 5)::numeric, 2)
FROM generate_series(0, 59999) AS i;

-- Seed data: Reviews (~8,000)
INSERT INTO reviews (product_id, customer_id, rating, body, created_at)
SELECT
    1 + (random() * 999)::integer,
    1 + (random() * 4999)::integer,
    1 + (random() * 4)::integer,
    CASE (i % 5)
        WHEN 0 THEN 'Great product, highly recommend!'
        WHEN 1 THEN 'Good value for the price.'
        WHEN 2 THEN 'Average quality, nothing special.'
        WHEN 3 THEN 'Below expectations, would not buy again.'
        WHEN 4 THEN 'Excellent! Exceeded all expectations.'
    END,
    now() - (random() * interval '180 days')
FROM generate_series(1, 8000) AS i;

-- Analyze tables for query planner
ANALYZE;
