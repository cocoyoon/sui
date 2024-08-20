// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use anyhow::{anyhow, Error};
use async_trait::async_trait;
use diesel::dsl::now;
use diesel::{Connection, OptionalExtension, QueryDsl, RunQueryDsl, SelectableHelper};
use diesel::{ExpressionMethods, TextExpressionMethods};
use sui_types::base_types::ObjectID;
use tracing::info;

use sui_indexer_builder::indexer_builder::{DataMapper, IndexerProgressStore, Persistent};
use sui_indexer_builder::sui_datasource::CheckpointTxnData;
use sui_indexer_builder::Task;
use sui_types::effects::TransactionEffectsAPI;
use sui_types::event::Event;
use sui_types::execution_status::ExecutionStatus;
use sui_types::full_checkpoint_content::CheckpointTransaction;

use crate::events::{
    MoveFlashLoanBorrowedEvent, MoveOrderCanceledEvent, MoveOrderFilledEvent,
    MoveOrderModifiedEvent, MoveOrderPlacedEvent, MovePriceAddedEvent,
};
use crate::metrics::DeepBookIndexerMetrics;
use crate::postgres_manager::PgPool;
use crate::schema::progress_store::{columns, dsl};
use crate::schema::{flashloans, order_fills, order_updates, pool_prices, sui_error_transactions};
use crate::{
    models, schema, Flashloan, OrderFill, OrderUpdate, OrderUpdateStatus, PoolPrice,
    ProcessedTxnData, SuiTxnError,
};

/// Persistent layer impl
#[derive(Clone)]
pub struct PgDeepbookPersistent {
    pool: PgPool,
}

impl PgDeepbookPersistent {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Persistent<ProcessedTxnData> for PgDeepbookPersistent {
    async fn write(&self, data: Vec<ProcessedTxnData>) -> Result<(), Error> {
        if data.is_empty() {
            return Ok(());
        }
        let connection = &mut self.pool.get()?;
        connection.transaction(|conn| {
            for d in data {
                match d {
                    ProcessedTxnData::OrderUpdate(t) => {
                        diesel::insert_into(order_updates::table)
                            .values(&t.to_db())
                            .on_conflict_do_nothing()
                            .execute(conn)?;
                    }
                    ProcessedTxnData::OrderFill(t) => {
                        diesel::insert_into(order_fills::table)
                            .values(&t.to_db())
                            .on_conflict_do_nothing()
                            .execute(conn)?;
                    }
                    ProcessedTxnData::Flashloan(t) => {
                        diesel::insert_into(flashloans::table)
                            .values(&t.to_db())
                            .on_conflict_do_nothing()
                            .execute(conn)?;
                    }
                    ProcessedTxnData::PoolPrice(t) => {
                        diesel::insert_into(pool_prices::table)
                            .values(&t.to_db())
                            .on_conflict_do_nothing()
                            .execute(conn)?;
                    }
                    ProcessedTxnData::Error(e) => {
                        diesel::insert_into(sui_error_transactions::table)
                            .values(&e.to_db())
                            .on_conflict_do_nothing()
                            .execute(conn)?;
                    }
                }
            }
            Ok(())
        })
    }
}

#[async_trait]
impl IndexerProgressStore for PgDeepbookPersistent {
    async fn load_progress(&self, task_name: String) -> anyhow::Result<u64> {
        let mut conn = self.pool.get()?;
        let cp: Option<models::ProgressStore> = dsl::progress_store
            .find(&task_name)
            .select(models::ProgressStore::as_select())
            .first(&mut conn)
            .optional()?;
        Ok(cp
            .ok_or(anyhow!("Cannot found progress for task {task_name}"))?
            .checkpoint as u64)
    }

    async fn save_progress(
        &mut self,
        task_name: String,
        checkpoint_number: u64,
    ) -> anyhow::Result<()> {
        let mut conn = self.pool.get()?;
        diesel::insert_into(schema::progress_store::table)
            .values(&models::ProgressStore {
                task_name,
                checkpoint: checkpoint_number as i64,
                // Target checkpoint and timestamp will only be written for new entries
                target_checkpoint: i64::MAX,
                // Timestamp is defaulted to current time in DB if None
                timestamp: None,
            })
            .on_conflict(dsl::task_name)
            .do_update()
            .set((
                columns::checkpoint.eq(checkpoint_number as i64),
                columns::timestamp.eq(now),
            ))
            .execute(&mut conn)?;
        Ok(())
    }

    async fn tasks(&self, prefix: &str) -> Result<Vec<Task>, anyhow::Error> {
        let mut conn = self.pool.get()?;
        // get all unfinished tasks
        let cp: Vec<models::ProgressStore> = dsl::progress_store
            // TODO: using like could be error prone, change the progress store schema to stare the task name properly.
            .filter(columns::task_name.like(format!("{prefix} - %")))
            .filter(columns::checkpoint.lt(columns::target_checkpoint))
            .order_by(columns::target_checkpoint.desc())
            .load(&mut conn)?;
        Ok(cp.into_iter().map(|d| d.into()).collect())
    }

    async fn register_task(
        &mut self,
        task_name: String,
        checkpoint: u64,
        target_checkpoint: i64,
    ) -> Result<(), anyhow::Error> {
        let mut conn = self.pool.get()?;
        diesel::insert_into(schema::progress_store::table)
            .values(models::ProgressStore {
                task_name,
                checkpoint: checkpoint as i64,
                target_checkpoint,
                // Timestamp is defaulted to current time in DB if None
                timestamp: None,
            })
            .execute(&mut conn)?;
        Ok(())
    }

    async fn update_task(&mut self, task: Task) -> Result<(), anyhow::Error> {
        let mut conn = self.pool.get()?;
        diesel::update(dsl::progress_store.filter(columns::task_name.eq(task.task_name)))
            .set((
                columns::checkpoint.eq(task.checkpoint as i64),
                columns::target_checkpoint.eq(task.target_checkpoint as i64),
                columns::timestamp.eq(now),
            ))
            .execute(&mut conn)?;
        Ok(())
    }
}

/// Data mapper impl
#[derive(Clone)]
pub struct SuiDeepBookDataMapper {
    pub metrics: DeepBookIndexerMetrics,
    pub package_id: ObjectID,
}

impl DataMapper<CheckpointTxnData, ProcessedTxnData> for SuiDeepBookDataMapper {
    fn map(
        &self,
        (data, checkpoint_num, timestamp_ms): CheckpointTxnData,
    ) -> Result<Vec<ProcessedTxnData>, Error> {
        // TODO: figure out a way to skip transactions that aren't deepbook transactions
        // self.metrics.total_sui_bridge_transactions.inc();
        // if !data
        //     .input_objects
        //     .iter()
        //     .any(|obj| obj.id() == self.package_id)
        // {
        //     return Ok(vec![]);
        // }

        match &data.events {
            Some(events) => {
                let processed_sui_events =
                    events.data.iter().try_fold(vec![], |mut result, ev| {
                        if let Some(data) = process_sui_event(
                            ev,
                            &data,
                            checkpoint_num,
                            timestamp_ms,
                            self.package_id,
                        )? {
                            result.push(data);
                        }
                        Ok::<_, anyhow::Error>(result)
                    })?;

                if !processed_sui_events.is_empty() {
                    info!(
                        "SUI: Extracted {} deepbook data entries for tx {}.",
                        processed_sui_events.len(),
                        data.transaction.digest()
                    );
                }
                Ok(processed_sui_events)
            }
            None => {
                if let ExecutionStatus::Failure { error, command } = data.effects.status() {
                    Ok(vec![ProcessedTxnData::Error(SuiTxnError {
                        tx_digest: *data.transaction.digest(),
                        sender: data.transaction.sender_address(),
                        timestamp_ms,
                        failure_status: error.to_string(),
                        cmd_idx: command.map(|idx| idx as u64),
                    })])
                } else {
                    Ok(vec![])
                }
            }
        }
    }
}

fn process_sui_event(
    ev: &Event,
    tx: &CheckpointTransaction,
    checkpoint: u64,
    timestamp_ms: u64,
    package_id: ObjectID,
) -> Result<Option<ProcessedTxnData>, anyhow::Error> {
    Ok(if ev.type_.address == *package_id {
        match ev.type_.name.as_str() {
            "OrderPlaced" => {
                info!("Observed Deepbook Order Placed {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderPlacedEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderUpdate(OrderUpdate {
                    digest: tx.transaction.digest().to_string(),
                    sender: tx.transaction.sender_address().to_string(),
                    checkpoint,
                    status: OrderUpdateStatus::Placed,
                    pool_id: move_event.pool_id.to_string(),
                    order_id: move_event.order_id,
                    client_order_id: move_event.client_order_id,
                    price: move_event.price,
                    is_bid: move_event.is_bid,
                    onchain_timestamp: move_event.expire_timestamp,
                    quantity: move_event.placed_quantity,
                    trader: move_event.trader.to_string(),
                    balance_manager_id: move_event.balance_manager_id.to_string(),
                }))
            }
            "OrderModified" => {
                info!("Observed Deepbook Order Modified {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderModifiedEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderUpdate(OrderUpdate {
                    digest: tx.transaction.digest().to_string(),
                    sender: tx.transaction.sender_address().to_string(),
                    checkpoint,
                    status: OrderUpdateStatus::Modified,
                    pool_id: move_event.pool_id.to_string(),
                    order_id: move_event.order_id,
                    client_order_id: move_event.client_order_id,
                    price: move_event.price,
                    is_bid: move_event.is_bid,
                    onchain_timestamp: move_event.timestamp,
                    quantity: move_event.new_quantity,
                    trader: move_event.trader.to_string(),
                    balance_manager_id: move_event.balance_manager_id.to_string(),
                }))
            }
            "OrderCanceled" => {
                info!("Observed Deepbook Order Canceled {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderCanceledEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderUpdate(OrderUpdate {
                    digest: tx.transaction.digest().to_string(),
                    sender: tx.transaction.sender_address().to_string(),
                    checkpoint,
                    status: OrderUpdateStatus::Canceled,
                    pool_id: move_event.pool_id.to_string(),
                    order_id: move_event.order_id,
                    client_order_id: move_event.client_order_id,
                    price: move_event.price,
                    is_bid: move_event.is_bid,
                    onchain_timestamp: move_event.timestamp,
                    quantity: move_event.base_asset_quantity_canceled,
                    trader: move_event.trader.to_string(),
                    balance_manager_id: move_event.balance_manager_id.to_string(),
                }))
            }
            "OrderFilled" => {
                info!("Observed Deepbook Order Filled {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderFilledEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderFill(OrderFill {
                    digest: tx.transaction.digest().to_string(),
                    sender: tx.transaction.sender_address().to_string(),
                    checkpoint,
                    pool_id: move_event.pool_id.to_string(),
                    maker_order_id: move_event.maker_order_id,
                    taker_order_id: move_event.taker_order_id,
                    maker_client_order_id: move_event.maker_client_order_id,
                    taker_client_order_id: move_event.taker_client_order_id,
                    price: move_event.price,
                    taker_is_bid: move_event.taker_is_bid,
                    base_quantity: move_event.base_quantity,
                    quote_quantity: move_event.quote_quantity,
                    maker_balance_manager_id: move_event.maker_balance_manager_id.to_string(),
                    taker_balance_manager_id: move_event.taker_balance_manager_id.to_string(),
                    onchain_timestamp: move_event.timestamp,
                }))
            }
            "FlashLoanBorrowed" => {
                info!("Observed Deepbook Flash Loan Borrowed {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveFlashLoanBorrowedEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::Flashloan(Flashloan {
                    digest: tx.transaction.digest().to_string(),
                    sender: tx.transaction.sender_address().to_string(),
                    checkpoint,
                    pool_id: move_event.pool_id.to_string(),
                    borrow_quantity: move_event.borrow_quantity,
                    borrow: true,
                    type_name: move_event.type_name.to_string(),
                }))
            }
            "PriceAdded" => {
                info!("Observed Deepbook Price Addition {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MovePriceAddedEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::PoolPrice(PoolPrice {
                    digest: tx.transaction.digest().to_string(),
                    sender: tx.transaction.sender_address().to_string(),
                    checkpoint,
                    target_pool: move_event.target_pool.to_string(),
                    conversion_rate: move_event.conversion_rate,
                    reference_pool: move_event.reference_pool.to_string(),
                }))
            }

            _ => {
                // todo: metrics.total_sui_bridge_txn_other.inc();
                None
            }
        }
    } else {
        None
    })
}
