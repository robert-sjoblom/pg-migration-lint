CREATE TABLE IF NOT EXISTS x (
    id bigint PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS y (
    id bigint PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS xy (
    x_id bigint NOT NULL REFERENCES x(id),
    y_id bigint NOT NULL REFERENCES y(id),
    PRIMARY KEY (x_id, y_id)
);
