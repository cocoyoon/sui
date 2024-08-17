// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::models::SuiProgressStore;
use crate::schema::sui_progress_store::txn_digest;
use crate::schema::{flashloans, modified_orders, placed_orders, sui_error_transactions};
use crate::{schema, ProcessedTxnData};
use diesel::{
    pg::PgConnection,
    r2d2::{ConnectionManager, Pool},
    Connection, ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, SelectableHelper,
};
use sui_types::digests::TransactionDigest;

pub(crate) type PgPool = Pool<ConnectionManager<PgConnection>>;

const SUI_PROGRESS_STORE_DUMMY_KEY: i32 = 1;

pub fn get_connection_pool(database_url: String) -> PgPool {
    let manager = ConnectionManager::<PgConnection>::new(database_url);
    Pool::builder()
        .test_on_check_out(true)
        .build(manager)
        .expect("Could not build Postgres DB connection pool")
}

// TODO: add retry logic
pub fn write(pool: &PgPool, txns: Vec<ProcessedTxnData>) -> Result<(), anyhow::Error> {
    if txns.is_empty() {
        return Ok(());
    }
    let (placed_orders, modified_orders, flahloans, errors) = txns.iter().fold(
        (vec![], vec![], vec![], vec![]),
        |(mut placed_orders, mut modified_orders, mut flashloans, mut errors), d| {
            match d {
                ProcessedTxnData::OrderPlaced(t) => {
                    placed_orders.push(t.to_db());
                }
                ProcessedTxnData::OrderModified(t) => {
                    modified_orders.push(t.to_db());
                }
                ProcessedTxnData::Flashloan(t) => {
                    flashloans.push(t.to_db());
                }
                ProcessedTxnData::Error(e) => errors.push(e.to_db()),
            }
            (placed_orders, modified_orders, flashloans, errors)
        },
    );

    let connection = &mut pool.get()?;
    connection.transaction(|conn| {
        diesel::insert_into(placed_orders::table)
            .values(&placed_orders)
            .on_conflict_do_nothing()
            .execute(conn)?;
        diesel::insert_into(modified_orders::table)
            .values(&modified_orders)
            .on_conflict_do_nothing()
            .execute(conn)?;
        diesel::insert_into(flashloans::table)
            .values(&flahloans)
            .on_conflict_do_nothing()
            .execute(conn)?;
        diesel::insert_into(sui_error_transactions::table)
            .values(&errors)
            .on_conflict_do_nothing()
            .execute(conn)
    })?;
    Ok(())
}

pub fn update_sui_progress_store(
    pool: &PgPool,
    tx_digest: TransactionDigest,
) -> Result<(), anyhow::Error> {
    let mut conn = pool.get()?;
    diesel::insert_into(schema::sui_progress_store::table)
        .values(&SuiProgressStore {
            id: SUI_PROGRESS_STORE_DUMMY_KEY,
            txn_digest: tx_digest.inner().to_vec(),
        })
        .on_conflict(schema::sui_progress_store::dsl::id)
        .do_update()
        .set(txn_digest.eq(tx_digest.inner().to_vec()))
        .execute(&mut conn)?;
    Ok(())
}

pub fn read_sui_progress_store(pool: &PgPool) -> anyhow::Result<Option<TransactionDigest>> {
    let mut conn = pool.get()?;
    let val: Option<SuiProgressStore> = crate::schema::sui_progress_store::dsl::sui_progress_store
        .select(SuiProgressStore::as_select())
        .first(&mut conn)
        .optional()?;
    match val {
        Some(val) => Ok(Some(TransactionDigest::try_from(
            val.txn_digest.as_slice(),
        )?)),
        None => Ok(None),
    }
}
