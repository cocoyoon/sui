// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use diesel::data_types::PgTimestamp;
use diesel::{Identifiable, Insertable, Queryable, Selectable};

use sui_indexer_builder::Task;

use crate::schema::{
    flashloans, modified_orders, placed_orders, progress_store, sui_error_transactions,
    sui_progress_store,
};

#[derive(Queryable, Selectable, Insertable, Identifiable, Debug)]
#[diesel(table_name = progress_store, primary_key(task_name))]
pub struct ProgressStore {
    pub task_name: String,
    pub checkpoint: i64,
    pub target_checkpoint: i64,
    pub timestamp: Option<PgTimestamp>,
}

impl From<ProgressStore> for Task {
    fn from(value: ProgressStore) -> Self {
        Self {
            task_name: value.task_name,
            checkpoint: value.checkpoint as u64,
            target_checkpoint: value.target_checkpoint as u64,
            // Ok to unwrap, timestamp is defaulted to now() in database
            timestamp: value.timestamp.expect("Timestamp not set").0 as u64,
        }
    }
}

#[derive(Queryable, Selectable, Insertable, Identifiable, Debug)]
#[diesel(table_name = sui_progress_store, primary_key(txn_digest))]
pub struct SuiProgressStore {
    pub id: i32, // Dummy value
    pub txn_digest: Vec<u8>,
}

#[derive(Queryable, Selectable, Insertable, Identifiable, Debug)]
#[diesel(table_name = placed_orders, primary_key(digest))]
pub struct OrderPlaced {
    pub digest: Vec<u8>,
    pub sender: Vec<u8>,
    pub checkpoint: i64,
    pub timestamp: i64,
    pub balance_manager_id: Vec<u8>,
    pub pool_id: Vec<u8>,
    pub order_id: Vec<u8>,
    pub client_order_id: Vec<u8>,
    pub trader: Vec<u8>,
    pub price: i64,
    pub is_bid: bool,
    pub placed_quantity: i64,
    pub expire_timestamp: i64,
}

#[derive(Queryable, Selectable, Insertable, Identifiable, Debug)]
#[diesel(table_name = modified_orders, primary_key(digest))]
pub struct OrderModified {
    pub digest: Vec<u8>,
    pub sender: Vec<u8>,
    pub checkpoint: i64,
    pub timestamp: i64,
    pub pool_id: Vec<u8>,
    pub order_id: Vec<u8>,
    pub client_order_id: Vec<u8>,
    pub price: i64,
    pub is_bid: bool,
    pub new_quantity: i64,
    pub onchain_timestamp: i64,
}

#[derive(Queryable, Selectable, Insertable, Identifiable, Debug)]
#[diesel(table_name = flashloans, primary_key(digest))]
pub struct Flashloan {
    pub digest: Vec<u8>,
    pub sender: Vec<u8>,
    pub checkpoint: i64,
    pub timestamp: i64,
    pub borrow: bool,
    pub pool_id: Vec<u8>,
    pub borrow_quantity: i64,
    pub type_name: String,
}

#[derive(Queryable, Selectable, Insertable, Identifiable, Debug)]
#[diesel(table_name = sui_error_transactions, primary_key(txn_digest))]
pub struct SuiErrorTransactions {
    pub txn_digest: Vec<u8>,
    pub sender_address: Vec<u8>,
    pub timestamp_ms: i64,
    pub failure_status: String,
    pub cmd_idx: Option<i64>,
}
