-- Your SQL goes here
CREATE TABLE IF NOT EXISTS modified_orders
(
    digest                      bytea        PRIMARY KEY,
    sender                      bytea        NOT NULL,
    checkpoint                  BIGINT       NOT NULL,
    timestamp                   TIMESTAMP    DEFAULT CURRENT_TIMESTAMP NOT NULL,
    pool_id                     bytea        NOT NULL,
    order_id                    bytea        NOT NULL,
    client_order_id             bytea        NOT NULL,
    price                       BIGINT       NOT NULL,
    is_bid                      BOOLEAN      NOT NULL,
    new_quantity                BIGINT       NOT NULL,
    onchain_timestamp           BIGINT       NOT NULL
);

CREATE TABLE IF NOT EXISTS placed_orders
(
    digest                      bytea        PRIMARY KEY,
    sender                      bytea        NOT NULL,
    checkpoint                  BIGINT       NOT NULL,
    timestamp                   TIMESTAMP    DEFAULT CURRENT_TIMESTAMP NOT NULL,
    balance_manager_id          bytea        NOT NULL,
    pool_id                     bytea        NOT NULL,
    order_id                    bytea        NOT NULL,
    client_order_id             bytea        NOT NULL,
    trader                      bytea        NOT NULL,
    price                       BIGINT       NOT NULL,
    is_bid                      BOOLEAN      NOT NULL,
    placed_quantity             BIGINT       NOT NULL,
    expire_timestamp            BIGINT       NOT NULL
);

CREATE TABLE IF NOT EXISTS flashloans
(
    digest                      bytea        PRIMARY KEY,
    sender                      bytea        NOT NULL,
    checkpoint                  BIGINT       NOT NULL,
    timestamp                   TIMESTAMP    DEFAULT CURRENT_TIMESTAMP NOT NULL,
    borrow                      BOOLEAN      NOT NULL,
    pool_id                     bytea        NOT NULL,
    borrow_quantity             BIGINT       NOT NULL,
    type_name                   TEXT         NOT NULL
);