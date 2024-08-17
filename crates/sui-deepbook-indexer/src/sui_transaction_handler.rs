// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::metrics::DeepBookIndexerMetrics;
use crate::postgres_manager::{update_sui_progress_store, write, PgPool};
use crate::types::RetrievedTransaction;
use crate::{Flashloan, OrderModified, OrderPlaced, ProcessedTxnData};
use anyhow::Result;
use futures::StreamExt;
use sui_types::digests::TransactionDigest;

use std::time::Duration;

// TODO: need to figure out where to add Deepbook events
// use sui_bridge::events::{
//     MoveTokenDepositedEvent, MoveTokenTransferApproved, MoveTokenTransferClaimed,
// };

use sui_json_rpc_types::SuiTransactionBlockEffectsAPI;

use mysten_metrics::metered_channel::{Receiver, ReceiverStream};
use sui_types::DEEPBOOK_ADDRESS;
use tracing::{error, info};

pub(crate) const COMMIT_BATCH_SIZE: usize = 10;

pub async fn handle_sui_transactions_loop(
    pg_pool: PgPool,
    rx: Receiver<(Vec<RetrievedTransaction>, Option<TransactionDigest>)>,
    metrics: DeepBookIndexerMetrics,
) {
    let checkpoint_commit_batch_size = std::env::var("COMMIT_BATCH_SIZE")
        .unwrap_or(COMMIT_BATCH_SIZE.to_string())
        .parse::<usize>()
        .unwrap();
    let mut stream = ReceiverStream::new(rx).ready_chunks(checkpoint_commit_batch_size);
    while let Some(batch) = stream.next().await {
        // unwrap: batch must not be empty
        let (txns, cursor) = batch.last().cloned().unwrap();
        let data = batch
            .into_iter()
            // TODO: letting it panic so we can capture errors, but we should handle this more gracefully
            .flat_map(|(chunk, _)| process_transactions(chunk, &metrics).unwrap())
            .collect::<Vec<_>>();

        // write batched transaction data to DB
        if !data.is_empty() {
            // unwrap: token_transfers is not empty
            // let last_ckp = txns.last().map(|tx| tx.checkpoint).unwrap_or_default();
            while let Err(err) = write(&pg_pool, data.clone()) {
                error!("Failed to write sui transactions to DB: {:?}", err);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            info!("Wrote {} deepbook transaction data to DB", data.len());
            // metrics.last_committed_sui_checkpoint.set(last_ckp as i64);
        }

        // update sui progress store using the latest cursor
        if let Some(cursor) = cursor {
            while let Err(err) = update_sui_progress_store(&pg_pool, cursor) {
                error!("Failed to update sui progress tore DB: {:?}", err);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            info!("Updated sui transaction cursor to {}", cursor);
        }
    }
    unreachable!("Channel closed unexpectedly");
}

fn process_transactions(
    txns: Vec<RetrievedTransaction>,
    metrics: &DeepBookIndexerMetrics,
) -> Result<Vec<ProcessedTxnData>> {
    txns.into_iter().try_fold(vec![], |mut result, tx| {
        result.append(&mut into_processed_txn_data(tx, metrics)?);
        Ok(result)
    })
}

pub fn into_processed_txn_data(
    tx: RetrievedTransaction,
    metrics: &DeepBookIndexerMetrics,
) -> Result<Vec<ProcessedTxnData>> {
    let mut processed_txn_data = Vec::new();
    let tx_digest = tx.tx_digest;
    let timestamp_ms = tx.timestamp_ms;
    let checkpoint_num = tx.checkpoint;
    let effects = tx.effects;
    for ev in tx.events.data {
        if ev.type_.address != DEEPBOOK_ADDRESS {
            continue;
        }
        match ev.type_.name.as_str() {
            "OrderPlaced" => {
                info!("Observed Deepbook Order Placed {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderPlacedEvent = bcs::from_bytes(&ev.bcs)?;
                processed_txn_data.push(ProcessedTxnData::OrderPlaced(OrderPlaced {
                    // TODO: map to correct fields
                }));
            }
            "OrderModified" => {
                info!("Observed Deepbook Order Modified {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderModifiedEvent = bcs::from_bytes(&ev.bcs)?;
                processed_txn_data.push(ProcessedTxnData::OrderModified(OrderModified {
                    // TODO: map to correct fields
                }));
            }
            "OrderInfoEvent" => {
                info!("Observed Deepbook Order Info {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderInfoEventEvent = bcs::from_bytes(&ev.bcs)?;
                processed_txn_data.push(ProcessedTxnData::OrderModified(OrderModified {
                    // TODO: map to correct fields
                }));
            }
            "OrderFilled" => {
                info!("Observed Deepbook Order Filled {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderFilledEvent = bcs::from_bytes(&ev.bcs)?;
                processed_txn_data.push(ProcessedTxnData::OrderModified(OrderModified {
                    // TODO: map to correct fields
                }));
            }
            "OrderCanceled" => {
                info!("Observed Deepbook Order Canceled {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveOrderCanceledEvent = bcs::from_bytes(&ev.bcs)?;
                processed_txn_data.push(ProcessedTxnData::OrderModified(OrderModified {
                    // TODO: map to correct fields
                }));
            }
            "FlashLoanBorrowed" => {
                info!("Observed Deepbook Flash Loan Borrowed {:?}", ev);
                // metrics.total_sui_token_deposited.inc();
                let move_event: MoveFlashLoanBorrowedEvent = bcs::from_bytes(&ev.bcs)?;
                processed_txn_data.push(ProcessedTxnData::Flashloan(Flashloan {
                    // TODO: map to correct fields
                }));
            }

            _ => {
                metrics.total_deepbook_transactions.inc();
            }
        }
    }
    if !processed_txn_data.is_empty() {
        info!(
            ?tx_digest,
            "SUI: Extracted {} Deepbook data entries",
            processed_txn_data.len(),
        );
    }
    Ok(processed_txn_data)
}
