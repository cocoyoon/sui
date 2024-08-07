// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::{Cursor, TransactionBlockFilter};
use crate::{
    data::{pg::bytea_literal, Conn, DbConnection},
    filter, inner_join, query,
    raw_query::RawQuery,
    types::{
        cursor::Page,
        digest::Digest,
        sui_address::SuiAddress,
        transaction_block::TransactionBlockKindInput,
        type_filter::{FqNameFilter, ModuleFilter},
    },
};
use diesel::{
    query_dsl::positional_order_dsl::PositionalOrderDsl, CombineDsl, ExpressionMethods,
    NullableExpressionMethods, QueryDsl,
};
use std::fmt::Write;
use sui_indexer::schema::checkpoints;

#[derive(Clone, Debug, Copy)]
pub(crate) struct TxBounds {
    /// The lower bound tx_sequence_number corresponding to the first tx_sequence_number of the
    /// lower checkpoint bound before applying `after` cursor and `scan_limit`.
    pub lo: u64,
    /// The upper bound tx_sequence_number corresponding to the last tx_sequence_number of the upper
    /// checkpoint bound before applying `before` cursor and `scan_limit`.
    pub hi: u64,
    pub after: Option<u64>,
    pub before: Option<u64>,
    pub scan_limit: Option<u64>,
    pub is_from_front: bool,
}

impl TxBounds {
    fn new(
        lo: u64,
        hi: u64,
        after: Option<u64>,
        before: Option<u64>,
        scan_limit: Option<u64>,
        is_from_front: bool,
    ) -> Self {
        Self {
            lo,
            hi,
            after,
            before,
            scan_limit,
            is_from_front,
        }
    }

    /// Determines the `tx_sequence_number` range from the checkpoint bounds for a transaction block
    /// query. If no checkpoint range is specified, the default is between 0 and the
    /// `checkpoint_viewed_at`. The corresponding `tx_sequence_number` range is fetched from db, and
    /// further adjusted by cursors and scan limit. If the after cursor exceeds rhs, or before
    /// cursor is below lhs, or other inconsistency, return None.
    pub(crate) fn query(
        conn: &mut Conn,
        after_cp: Option<u64>,
        at_cp: Option<u64>,
        before_cp: Option<u64>,
        checkpoint_viewed_at: u64,
        scan_limit: Option<u64>,
        page: &Page<Cursor>,
    ) -> Result<Option<Self>, diesel::result::Error> {
        // Increment the lower bound checkpoint by 1 so we can select its min tx_sequence_number
        // inclusively.
        let lo_cp = max_option([after_cp.map(|x| x.saturating_add(1)), at_cp]).unwrap_or(0);
        // Assumes that `before_cp` is greater than 0. In the `TransactionBlock::paginate` flow, we
        // check if `before_cp` is 0, and if so, short-circuit and produce no results. Decrements
        // the upper bound checkpoint by 1 so we can select its max tx_sequence_number inclusively.
        let hi_cp = min_option([
            before_cp.map(|x| x.saturating_sub(1)),
            at_cp,
            Some(checkpoint_viewed_at),
        ])
        .unwrap();

        use checkpoints::dsl;

        let from_db: Vec<(Option<i64>, Option<i64>)> = conn.results(move || {
            // Construct a UNION ALL query ordered on `sequence_number` to get the tx ranges for the
            // checkpoint range.
            dsl::checkpoints
                .select((
                    dsl::sequence_number.nullable(),
                    dsl::network_total_transactions.nullable(),
                ))
                .filter(dsl::sequence_number.eq(lo_cp.saturating_sub(1) as i64))
                .union(
                    dsl::checkpoints
                        .select((
                            dsl::sequence_number.nullable(),
                            dsl::network_total_transactions.nullable() - 1,
                        ))
                        .filter(dsl::sequence_number.eq(hi_cp as i64)),
                )
                .positional_order_by(1) // order by checkpoint's sequence number, which is the first column
        })?;

        // Expect exactly two rows, returning early if not.
        let [(Some(db_lo_cp), Some(lo)), (Some(db_hi_cp), Some(hi))] = from_db.as_slice() else {
            return Ok(None);
        };

        if *db_lo_cp as u64 != lo_cp.saturating_sub(1) || *db_hi_cp as u64 != hi_cp {
            return Ok(None);
        }

        let lo = if lo_cp == 0 { 0 } else { *lo as u64 };
        let hi = *hi as u64;

        if page.after().is_some_and(|x| x.tx_sequence_number >= hi)
            || page.before().is_some_and(|x| x.tx_sequence_number <= lo)
        {
            return Ok(None);
        }

        Ok(Some(Self::new(
            lo,
            hi,
            page.after().map(|x| x.tx_sequence_number),
            page.before().map(|x| x.tx_sequence_number),
            scan_limit,
            page.is_from_front(),
        )))
    }

    // Returns the larger of the current checkpoint lower bound, or the `after` checkpoint cursor.
    fn tx_lo(&self) -> u64 {
        max_option([self.after, Some(self.lo)]).unwrap()
    }

    // Returns the smaller of the current checkpoint upper bound, or the `before` checkpoint cursor.
    fn tx_hi(&self) -> u64 {
        min_option([self.before, Some(self.hi)]).unwrap()
    }

    /// The lower bound `tx_sequence_number` of the range to scan within. When paginating forwards,
    /// this is the min `tx_sequence_number` from the lesser checkpoint of the scanning range. If
    /// scanning backwards, then this is the max `tx_sequence_number` between the former and the
    /// upper `tx_sequence_number` bound adjusted by `scan_limit`.
    pub(crate) fn scan_lo(&self) -> u64 {
        let adjusted_lo = self.tx_lo();

        if self.is_from_front {
            adjusted_lo
        } else if let Some(scan_limit) = self.scan_limit {
            adjusted_lo.max(
                self.tx_hi()
                    .saturating_sub(scan_limit)
                    // We encounter an off-by-one error when the `before` cursor is not provided, so
                    // we add 1 to counteract this
                    .saturating_add(self.before.is_none() as u64),
            )
        } else {
            adjusted_lo
        }
    }

    /// The upper bound `tx_sequence_number` of the range to scan within.  When paginating backwards,
    /// this is the max `tx_sequence_number` from the upper checkpoint of the scanning range. If
    /// scanning forwards, then this is the min `tx_sequence_number` between the former and the
    /// lesser `tx_sequence_number` bound adjusted by `scan_limit`.
    pub(crate) fn scan_hi(&self) -> u64 {
        let adjusted_hi = self.tx_hi();

        if !self.is_from_front {
            adjusted_hi
        } else if let Some(scan_limit) = self.scan_limit {
            // We encounter an off-by-one error when the `after` cursor is not provided, so we
            // subtract 1 to counteract this
            adjusted_hi.min(
                self.tx_lo()
                    .saturating_add(scan_limit)
                    .saturating_sub(self.after.is_none() as u64),
            )
        } else {
            adjusted_hi
        }
    }

    /// If the query result does not have a previous page, check whether the current page's starting
    /// tip is within the scanning range.
    pub(crate) fn scan_has_prev_page(&self) -> bool {
        self.lo < self.scan_lo()
    }

    /// If the query result does not have a next page, check whether the current page's ending tip
    /// is within the scanning range.
    pub(crate) fn scan_has_next_page(&self) -> bool {
        self.scan_hi() < self.hi
    }
}

/// Determines the maximum value in an arbitrary number of Option<impl Ord>.
fn max_option<T: Ord>(xs: impl IntoIterator<Item = Option<T>>) -> Option<T> {
    xs.into_iter().flatten().max()
}

/// Determines the minimum value in an arbitrary number of Option<impl Ord>.
fn min_option<T: Ord>(xs: impl IntoIterator<Item = Option<T>>) -> Option<T> {
    xs.into_iter().flatten().min()
}

/// Constructs a `RawQuery` as a join over all relevant side tables, filtered on their own filter
/// condition, plus optionally a sender, plus optionally tx/cp bounds.
pub(crate) fn subqueries(filter: &TransactionBlockFilter, tx_bounds: TxBounds) -> Option<RawQuery> {
    let sender = filter.sign_address;

    let mut subqueries = vec![];

    if let Some(f) = &filter.function {
        subqueries.push(match f {
            FqNameFilter::ByModule(filter) => match filter {
                ModuleFilter::ByPackage(p) => ("tx_calls_pkg", select_pkg(p, sender, tx_bounds)),
                ModuleFilter::ByModule(p, m) => {
                    ("tx_calls_mod", select_mod(p, m.clone(), sender, tx_bounds))
                }
            },
            FqNameFilter::ByFqName(p, m, n) => (
                "tx_calls_fun",
                select_fun(p, m.clone(), n.clone(), sender, tx_bounds),
            ),
        });
    }
    if let Some(kind) = &filter.kind {
        subqueries.push(("tx_kinds", select_kind(*kind, sender, tx_bounds)));
    }
    if let Some(recv) = &filter.recv_address {
        subqueries.push(("tx_recipients", select_recipient(recv, sender, tx_bounds)));
    }
    if let Some(input) = &filter.input_object {
        subqueries.push(("tx_input_objects", select_input(input, sender, tx_bounds)));
    }
    if let Some(changed) = &filter.changed_object {
        subqueries.push((
            "tx_changed_objects",
            select_changed(changed, sender, tx_bounds),
        ));
    }
    if let Some(sender) = &filter.explicit_sender() {
        subqueries.push(("tx_senders", select_sender(sender, tx_bounds)));
    }
    if let Some(txs) = &filter.transaction_ids {
        subqueries.push(("tx_digests", select_ids(txs, tx_bounds)));
    }

    let Some((_, mut subquery)) = subqueries.pop() else {
        return None;
    };

    if !subqueries.is_empty() {
        subquery = query!("SELECT tx_sequence_number FROM ({}) AS initial", subquery);
        while let Some((alias, subselect)) = subqueries.pop() {
            subquery = inner_join!(subquery, alias => subselect, using: ["tx_sequence_number"]);
        }
    }

    Some(subquery)
}

fn select_tx(sender: Option<SuiAddress>, bound: TxBounds, from: &str) -> RawQuery {
    let mut query = filter!(
        query!(format!("SELECT tx_sequence_number FROM {from}")),
        format!(
            "{} <= tx_sequence_number AND tx_sequence_number <= {}",
            bound.scan_lo(),
            bound.scan_hi()
        )
    );

    if let Some(sender) = sender {
        query = filter!(
            query,
            format!("sender = {}", bytea_literal(sender.as_slice()))
        );
    }

    query
}

fn select_pkg(pkg: &SuiAddress, sender: Option<SuiAddress>, bound: TxBounds) -> RawQuery {
    filter!(
        select_tx(sender, bound, "tx_calls_pkg"),
        format!("package = {}", bytea_literal(pkg.as_slice()))
    )
}

fn select_mod(
    pkg: &SuiAddress,
    mod_: String,
    sender: Option<SuiAddress>,
    bound: TxBounds,
) -> RawQuery {
    filter!(
        select_tx(sender, bound, "tx_calls_mod"),
        format!(
            "package = {} and module = {{}}",
            bytea_literal(pkg.as_slice())
        ),
        mod_
    )
}

fn select_fun(
    pkg: &SuiAddress,
    mod_: String,
    fun: String,
    sender: Option<SuiAddress>,
    bound: TxBounds,
) -> RawQuery {
    filter!(
        select_tx(sender, bound, "tx_calls_fun"),
        format!(
            "package = {} AND module = {{}} AND func = {{}}",
            bytea_literal(pkg.as_slice()),
        ),
        mod_,
        fun
    )
}

/// Returns a RawQuery that selects transactions of a specific kind. If SystemTX is specified, we
/// ignore the `sender`. If ProgrammableTX is specified, we filter against the `tx_kinds` table if
/// no `sender` is provided; otherwise, we just query the `tx_senders` table. Other combinations, in
/// particular when kind is SystemTx and sender is specified and not 0x0, are inconsistent and will
/// not produce any results. These inconsistent cases are expected to be checked for before this is
/// called.
fn select_kind(
    kind: TransactionBlockKindInput,
    sender: Option<SuiAddress>,
    bound: TxBounds,
) -> RawQuery {
    match (kind, sender) {
        // We can simplify the query to just the `tx_senders` table if ProgrammableTX and sender is
        // specified.
        (TransactionBlockKindInput::ProgrammableTx, Some(sender)) => select_sender(&sender, bound),
        // Otherwise, we can ignore the sender always, and just query the `tx_kinds` table.
        _ => filter!(
            select_tx(None, bound, "tx_kinds"),
            format!("tx_kind = {}", kind as i16)
        ),
    }
}

fn select_sender(sender: &SuiAddress, bound: TxBounds) -> RawQuery {
    select_tx(Some(*sender), bound, "tx_senders")
}

fn select_recipient(recv: &SuiAddress, sender: Option<SuiAddress>, bound: TxBounds) -> RawQuery {
    filter!(
        select_tx(sender, bound, "tx_recipients"),
        format!("recipient = {}", bytea_literal(recv.as_slice()))
    )
}

fn select_input(input: &SuiAddress, sender: Option<SuiAddress>, bound: TxBounds) -> RawQuery {
    filter!(
        select_tx(sender, bound, "tx_input_objects"),
        format!("object_id = {}", bytea_literal(input.as_slice()))
    )
}

fn select_changed(changed: &SuiAddress, sender: Option<SuiAddress>, bound: TxBounds) -> RawQuery {
    filter!(
        select_tx(sender, bound, "tx_changed_objects"),
        format!("object_id = {}", bytea_literal(changed.as_slice()))
    )
}

fn select_ids(ids: &Vec<Digest>, bound: TxBounds) -> RawQuery {
    let query = select_tx(None, bound, "tx_digests");
    if ids.is_empty() {
        filter!(query, "1=0")
    } else {
        let mut inner = String::new();
        let mut prefix = "tx_digest IN (";
        for id in ids {
            write!(&mut inner, "{prefix}{}", bytea_literal(id.as_slice())).unwrap();
            prefix = ", ";
        }
        inner.push(')');
        filter!(query, inner)
    }
}
