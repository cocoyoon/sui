// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use sui_types::{
    base_types::{ObjectID, SuiAddress},
    TypeTag,
};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct MoveOrderFilledEvent {
    pub pool_id: ObjectID,
    pub maker_order_id: u128,
    pub taker_order_id: u128,
    pub maker_client_order_id: u64,
    pub taker_client_order_id: u64,
    pub price: u64,
    pub taker_is_bid: bool,
    pub base_quantity: u64,
    pub quote_quantity: u64,
    pub maker_balance_manager_id: ObjectID,
    pub taker_balance_manager_id: ObjectID,
    pub timestamp: u64,
}

pub struct MoveOrderCanceledEvent {
    pub pool_id: ObjectID,
    pub order_id: u128,
    pub client_order_id: u64,
    pub price: u64,
    pub is_bid: bool,
    pub base_asset_quantity_canceled: u64,
    pub timestamp: u64,
}
pub struct MoveOrderModifiedEvent {
    pub pool_id: ObjectID,
    pub order_id: u128,
    pub client_order_id: u64,
    pub price: u64,
    pub is_bid: bool,
    pub new_quantity: u64,
    pub timestamp: u64,
}
pub struct MoveOrderPlacedEvent {
    pub balance_manager_id: ObjectID,
    pub pool_id: ObjectID,
    pub order_id: u128,
    pub client_order_id: u64,
    pub trader: SuiAddress,
    pub price: u64,
    pub is_bid: bool,
    pub placed_quantity: u64,
    pub expire_timestamp: u64,
}
pub struct MovePriceAddedEvent {
    pub conversion_rate: u64,
    pub timestamp: u64,
    pub is_base_conversion: bool,
    pub reference_pool: ObjectID,
    pub target_pool: ObjectID,
}
pub struct MoveFlashLoanBorrowedEvent {
    pub pool_id: ObjectID,
    pub borrow_quantity: u64,
    pub type_name: TypeTag,
}
