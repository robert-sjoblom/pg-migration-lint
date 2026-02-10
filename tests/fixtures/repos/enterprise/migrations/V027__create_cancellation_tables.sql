-- Cancellation feedback and trial conversion tracking

CREATE TABLE cancellation_feedback (
    id uuid PRIMARY KEY,
    order_id bigint NOT NULL REFERENCES orders(id),
    reason varchar(100) NOT NULL,
    details text,
    submitted_by bigint REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE trial_conversions (
    id uuid PRIMARY KEY,
    account_id bigint NOT NULL,
    converted_from varchar(50) NOT NULL,
    converted_to varchar(50) NOT NULL,
    converted_at timestamptz NOT NULL DEFAULT now()
);
