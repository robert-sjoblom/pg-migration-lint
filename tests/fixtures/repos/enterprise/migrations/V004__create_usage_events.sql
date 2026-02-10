-- Usage event ledger with composite primary key

CREATE TABLE usage_events (
    id uuid NOT NULL,
    account_id bigint NOT NULL,
    event_type varchar(100) NOT NULL,
    created timestamp(6) NOT NULL,
    price numeric(12,2),
    status varchar(100) NOT NULL DEFAULT 'CREATED',
    quantity smallint NOT NULL DEFAULT 1,
    kafka_offset bigint NOT NULL,
    kafka_partition bigint NOT NULL,
    PRIMARY KEY (event_type, kafka_partition, kafka_offset)
);
