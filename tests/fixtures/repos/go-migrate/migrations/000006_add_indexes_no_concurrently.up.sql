-- PGM001: CREATE INDEX without CONCURRENTLY on existing tables
CREATE INDEX idx_users_email ON users (email);
CREATE INDEX idx_orders_status ON orders (status);
