// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0
// @generated automatically by Diesel CLI.

diesel::table! {
    progress_store (task_name) {
        task_name -> Text,
        checkpoint -> Int8,
        target_checkpoint -> Int8,
        timestamp -> Nullable<Timestamp>,
    }
}

diesel::table! {
    sui_progress_store (id) {
        id -> Int4,
        txn_digest -> Bytea,
    }
}

diesel::table! {
    sui_error_transactions (txn_digest) {
        txn_digest -> Bytea,
        sender_address -> Bytea,
        timestamp_ms -> Int8,
        failure_status -> Text,
        cmd_idx -> Nullable<Int8>,
    }
}

diesel::table! {
    modified_orders (digest) {
        digest -> Bytea,
        sender -> Bytea,
        checkpoint -> Int8,
        timestamp -> Int8,
        pool_id -> Bytea,
        order_id -> Bytea,
        client_order_id -> Bytea,
        price -> Int8,
        is_bid -> Bool,
        new_quantity -> Int8,
        onchain_timestamp -> Int8
    }
}

diesel::table! {
    placed_orders (digest) {
        digest -> Bytea,
        sender -> Bytea,
        checkpoint -> Int8,
        timestamp -> Int8,
        balance_manager_id -> Bytea,
        pool_id -> Bytea,
        order_id -> Bytea,
        client_order_id -> Bytea,
        trader -> Bytea,
        price -> Int8,
        is_bid -> Bool,
        placed_quantity -> Int8,
        expire_timestamp -> Int8
    }
}

diesel::table! {
    flashloans (digest) {
        digest -> Bytea,
        sender -> Bytea,
        checkpoint -> Int8,
        timestamp -> Int8,
        borrow -> Bool,
        pool_id -> Bytea,
        borrow_quantity -> Int8,
        type_name -> Text,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    progress_store,
    sui_error_transactions,
    sui_progress_store,
    modified_orders,
    placed_orders,
    flashloans
);
