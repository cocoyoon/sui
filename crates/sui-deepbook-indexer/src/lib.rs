// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use sui_types::base_types::{SuiAddress, TransactionDigest};

use crate::models::Flashloan as DBFlashloan;
use crate::models::OrderPlaced as DBOrderPlaced;
use crate::models::{OrderModified as DBOrderModified, SuiErrorTransactions};

pub mod config;
pub mod metrics;
pub mod models;
pub mod postgres_manager;
pub mod schema;
pub mod sui_transaction_handler;
pub mod sui_transaction_queries;
pub mod types;

pub mod sui_deepbook_indexer;

#[derive(Clone)]
pub enum ProcessedTxnData {
    Flashloan(Flashloan),
    OrderPlaced(OrderPlaced),
    OrderModified(OrderModified),
    Error(SuiTxnError),
}

#[derive(Clone)]
pub struct SuiTxnError {
    tx_digest: TransactionDigest,
    sender: SuiAddress,
    timestamp_ms: u64,
    failure_status: String,
    cmd_idx: Option<u64>,
}

#[derive(Clone)]
pub struct OrderPlaced {
    digest: Vec<u8>,
    sender: Vec<u8>,
    checkpoint: u64,
    timestamp: u8,
    balance_manager_id: Vec<u8>,
    pool_id: Vec<u8>,
    order_id: Vec<u8>,
    client_order_id: Vec<u8>,
    trader: Vec<u8>,
    price: u64,
    is_bid: bool,
    placed_quantity: u64,
    expire_timestamp: u64,
}

impl OrderPlaced {
    fn to_db(&self) -> DBOrderPlaced {
        DBOrderPlaced {
            digest: self.digest.clone(),
            sender: self.sender.clone(),
            checkpoint: self.checkpoint as i64,
            timestamp: self.timestamp as i64,
            balance_manager_id: self.balance_manager_id.clone(),
            pool_id: self.pool_id.clone(),
            order_id: self.order_id.clone(),
            client_order_id: self.client_order_id.clone(),
            trader: self.trader.clone(),
            price: self.price as i64,
            is_bid: self.is_bid,
            placed_quantity: self.placed_quantity as i64,
            expire_timestamp: self.expire_timestamp as i64,
        }
    }
}

#[derive(Clone)]
pub struct OrderModified {
    digest: Vec<u8>,
    sender: Vec<u8>,
    checkpoint: u64,
    timestamp: u8,
    pool_id: Vec<u8>,
    order_id: Vec<u8>,
    client_order_id: Vec<u8>,
    price: u64,
    is_bid: bool,
    new_quantity: u64,
    onchain_timestamp: u64,
}

impl OrderModified {
    fn to_db(&self) -> DBOrderModified {
        DBOrderModified {
            digest: self.digest.clone(),
            sender: self.sender.clone(),
            checkpoint: self.checkpoint as i64,
            timestamp: self.timestamp as i64,
            pool_id: self.pool_id.clone(),
            order_id: self.order_id.clone(),
            client_order_id: self.client_order_id.clone(),
            price: self.price as i64,
            is_bid: self.is_bid,
            new_quantity: self.new_quantity as i64,
            onchain_timestamp: self.onchain_timestamp as i64,
        }
    }
}

#[derive(Clone)]
pub struct Flashloan {
    digest: Vec<u8>,
    sender: Vec<u8>,
    checkpoint: u64,
    timestamp: u8,
    borrow: bool,
    pool_id: Vec<u8>,
    borrow_quantity: u64,
    type_name: String,
}

impl Flashloan {
    fn to_db(&self) -> DBFlashloan {
        DBFlashloan {
            digest: self.digest.clone(),
            sender: self.sender.clone(),
            checkpoint: self.checkpoint as i64,
            timestamp: self.timestamp as i64,
            borrow: self.borrow,
            pool_id: self.pool_id.clone(),
            borrow_quantity: self.borrow_quantity as i64,
            type_name: self.type_name.clone(),
        }
    }
}

impl SuiTxnError {
    fn to_db(&self) -> SuiErrorTransactions {
        SuiErrorTransactions {
            txn_digest: self.tx_digest.inner().to_vec(),
            sender_address: self.sender.to_vec(),
            timestamp_ms: self.timestamp_ms as i64,
            failure_status: self.failure_status.clone(),
            cmd_idx: self.cmd_idx.map(|idx| idx as i64),
        }
    }
}
