-- Base system tables for the SaaS Project Management & Billing Platform

CREATE TABLE billing_lock (
    locked boolean NOT NULL,
    lock_granted timestamp(6)
);

CREATE TABLE users (
    id bigint PRIMARY KEY,
    display_name varchar(100),
    username varchar(100),
    email varchar(255),
    created timestamp(6) NOT NULL
);
