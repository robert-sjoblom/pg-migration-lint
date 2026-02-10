-- External integration (connector) tables

CREATE TABLE connector_catalog (
    id serial PRIMARY KEY,
    connector_name varchar(100) NOT NULL UNIQUE,
    api_version varchar(20),
    active boolean NOT NULL DEFAULT true
);

CREATE TABLE connector_requests (
    id uuid PRIMARY KEY,
    account_id bigint NOT NULL,
    connector_id integer NOT NULL REFERENCES connector_catalog(id),
    request_type varchar(50) NOT NULL,
    status varchar(50) NOT NULL DEFAULT 'PENDING',
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE connector_articles (
    id serial PRIMARY KEY,
    connector_id integer NOT NULL REFERENCES connector_catalog(id),
    external_article_id varchar(200) NOT NULL,
    product_id integer REFERENCES products(id),
    synced_at timestamptz
);
