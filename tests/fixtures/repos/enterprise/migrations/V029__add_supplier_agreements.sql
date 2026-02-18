-- Supplier agreement tracking
-- Note: FK without covering index on connector_articles triggers PGM501

CREATE TABLE supplier_agreements (
    id uuid PRIMARY KEY,
    account_id bigint NOT NULL,
    supplier_name varchar(200) NOT NULL,
    agreement_type varchar(50) NOT NULL,
    status varchar(50) NOT NULL DEFAULT 'ACTIVE',
    valid_from date NOT NULL,
    valid_until date,
    created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE connector_articles ADD COLUMN supplier_agreement_id uuid;
ALTER TABLE connector_articles ADD CONSTRAINT fk_articles_supplier
    FOREIGN KEY (supplier_agreement_id) REFERENCES supplier_agreements(id);
