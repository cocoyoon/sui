// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use anyhow::{anyhow, Error};
use async_trait::async_trait;
use diesel::dsl::now;
use diesel::{Connection, OptionalExtension, QueryDsl, RunQueryDsl, SelectableHelper};
use diesel::{ExpressionMethods, TextExpressionMethods};
use tracing::info;

use sui_bridge::events::{
    MoveTokenDepositedEvent, MoveTokenTransferApproved, MoveTokenTransferClaimed,
};
use sui_indexer_builder::indexer_builder::{DataMapper, IndexerProgressStore, Persistent};
use sui_indexer_builder::sui_datasource::CheckpointTxnData;
use sui_indexer_builder::Task;
use sui_types::effects::TransactionEffectsAPI;
use sui_types::event::Event;
use sui_types::execution_status::ExecutionStatus;
use sui_types::full_checkpoint_content::CheckpointTransaction;
use sui_types::{DEEPBOOK_ADDRESS, DEEPBOOK_PACKAGE_ID};

use crate::metrics::DeepBookIndexerMetrics;
use crate::postgres_manager::PgPool;
use crate::schema::progress_store::{columns, dsl};
use crate::schema::{
    flashloans, modified_orders, placed_orders, progress_store, sui_error_transactions,
};

use crate::{models, schema, Flashloan, OrderModified, OrderPlaced, ProcessedTxnData, SuiTxnError};

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
                    ProcessedTxnData::OrderPlaced(t) => {
                        diesel::insert_into(placed_orders::table)
                            .values(&t.to_db())
                            .on_conflict_do_nothing()
                            .execute(conn)?;
                    }
                    ProcessedTxnData::OrderModified(t) => {
                        diesel::insert_into(modified_orders::table)
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
}

impl DataMapper<CheckpointTxnData, ProcessedTxnData> for SuiDeepBookDataMapper {
    fn map(
        &self,
        (data, checkpoint_num, timestamp_ms): CheckpointTxnData,
    ) -> Result<Vec<ProcessedTxnData>, Error> {
        // self.metrics.total_sui_bridge_transactions.inc();
        if !data
            .input_objects
            .iter()
            .any(|obj| obj.id() == DEEPBOOK_PACKAGE_ID)
        {
            return Ok(vec![]);
        }

        match &data.events {
            Some(events) => {
                let token_transfers = events.data.iter().try_fold(vec![], |mut result, ev| {
                    if let Some(data) = process_sui_event(ev, &data, checkpoint_num, timestamp_ms)?
                    {
                        result.push(data);
                    }
                    Ok::<_, anyhow::Error>(result)
                })?;

                if !token_transfers.is_empty() {
                    info!(
                        "SUI: Extracted {} deepbook data entries for tx {}.",
                        token_transfers.len(),
                        data.transaction.digest()
                    );
                }
                Ok(token_transfers)
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
) -> Result<Option<ProcessedTxnData>, anyhow::Error> {
    Ok(if ev.type_.address == DEEPBOOK_ADDRESS {
        match ev.type_.name.as_str() {
            "OrderPlaced" => {
                info!("Observed Deepbook Order Placed {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderPlacedEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderPlaced(OrderPlaced {
                    // TODO: map to correct fields
                }))
            }
            "OrderModified" => {
                info!("Observed Deepbook Order Modified {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderModifiedEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderModified(OrderModified {
                    // TODO: map to correct fields
                }))
            }
            "OrderInfoEvent" => {
                info!("Observed Deepbook Order Info {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderInfoEventEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderModified(OrderModified {
                    // TODO: map to correct fields
                }))
            }
            "OrderFilled" => {
                info!("Observed Deepbook Order Filled {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderFilledEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderModified(OrderModified {
                    // TODO: map to correct fields
                }))
            }
            "OrderCanceled" => {
                info!("Observed Deepbook Order Canceled {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderCanceledEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::OrderModified(OrderModified {
                    // TODO: map to correct fields
                }))
            }
            "FlashLoanBorrowed" => {
                info!("Observed Deepbook Flash Loan Borrowed {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveFlashLoanBorrowedEvent = bcs::from_bytes(&ev.contents)?;
                Some(ProcessedTxnData::Flashloan(Flashloan {
                    // TODO: map to correct fields
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
