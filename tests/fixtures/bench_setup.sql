CREATE TABLE IF NOT EXISTS test_items (
    id serial PRIMARY KEY,
    name text NOT NULL,
    value numeric(10,2)
);
INSERT INTO test_items (name, value) SELECT 'item_' || i, (random() * 100)::numeric(10,2) FROM generate_series(1, 50) i;
