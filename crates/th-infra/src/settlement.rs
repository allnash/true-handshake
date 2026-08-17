//! Escrow, as a mock ledger behind the `SettlementProvider` port.
//!
//! No real money moves here. What does exist is the full state machine a real
//! processor would drive — hold, release, refund, idempotent under retry — so
//! that swapping in a PSP adapter later is a new `impl SettlementProvider` and
//! nothing else. When that happens `evidence_tier()` returns `Observed`, the
//! receipt gains a stronger evidence tier, and the domain does not change at all.
//!
//! Entries are double-entry: every movement writes two rows that sum to zero, so
//! "the ledger balances" is a query rather than a belief.

use async_trait::async_trait;
use sqlx::{PgPool, Row};
use th_app::{AppError, Result, SettlementProvider, SettlementState};
use th_domain::{DealId, EvidenceTier, Money};
use time::OffsetDateTime;
use tracing::info;

fn storage<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Settlement(e.to_string())
}

pub struct MockEscrow {
    pool: PgPool,
}

impl MockEscrow {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn state_of(&self, handle: &str) -> Result<SettlementState> {
        let row = sqlx::query("select state from ledger_holds where handle = $1")
            .bind(handle)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| AppError::Settlement(format!("unknown handle {handle}")))?;

        Ok(match row.try_get::<String, _>("state").map_err(storage)?.as_str() {
            "held" => SettlementState::Held,
            "released" => SettlementState::Released,
            "refunded" => SettlementState::Refunded,
            _ => SettlementState::Declared,
        })
    }

    /// Move the held amount out of escrow, exactly once.
    async fn settle(
        &self,
        handle: &str,
        to_state: &str,
        credit_account: &str,
        memo: &str,
    ) -> Result<SettlementState> {
        let mut tx = self.pool.begin().await.map_err(storage)?;

        let row = sqlx::query(
            "select deal_id, currency, minor_units, state from ledger_holds \
             where handle = $1 for update",
        )
        .bind(handle)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| AppError::Settlement(format!("unknown handle {handle}")))?;

        let current: String = row.try_get("state").map_err(storage)?;

        // Idempotent: a retried release is a no-op, not a second payout.
        if current == to_state {
            tx.commit().await.map_err(storage)?;
            return self.state_of(handle).await;
        }
        if current != "held" {
            return Err(AppError::Settlement(format!(
                "cannot {to_state} funds that are {current}"
            )));
        }

        let deal_id: uuid::Uuid = row.try_get("deal_id").map_err(storage)?;
        let currency: String = row.try_get("currency").map_err(storage)?;
        let minor_units: i64 = row.try_get("minor_units").map_err(storage)?;
        let now = OffsetDateTime::now_utc();

        sqlx::query("update ledger_holds set state = $1, settled_at = $2 where handle = $3")
            .bind(to_state)
            .bind(now)
            .bind(handle)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;

        for (account, delta) in [
            ("escrow".to_string(), -minor_units),
            (format!("{credit_account}:{deal_id}"), minor_units),
        ] {
            sqlx::query(
                "insert into ledger_entries (handle, account, currency, minor_units, at, memo) \
                 values ($1,$2,$3,$4,$5,$6)",
            )
            .bind(handle)
            .bind(&account)
            .bind(&currency)
            .bind(delta)
            .bind(now)
            .bind(memo)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }

        tx.commit().await.map_err(storage)?;
        info!(handle, to_state, "escrow settled");
        self.state_of(handle).await
    }

    /// Sums every entry for a handle. Should always be zero.
    pub async fn balance_check(&self, handle: &str) -> Result<i64> {
        let sum: Option<i64> = sqlx::query_scalar(
            "select coalesce(sum(minor_units), 0) from ledger_entries where handle = $1",
        )
        .bind(handle)
        .fetch_one(&self.pool)
        .await
        .map_err(storage)?;
        Ok(sum.unwrap_or(0))
    }
}

#[async_trait]
impl SettlementProvider for MockEscrow {
    fn id(&self) -> &'static str {
        "mock-escrow"
    }

    /// v1 records signed claims about money, not observed transfers. Saying so
    /// in the type is the whole reason this method exists.
    fn evidence_tier(&self) -> EvidenceTier {
        EvidenceTier::Attested
    }

    async fn hold(&self, deal_id: DealId, amount: &Money) -> Result<String> {
        // One hold per deal. If a funding command is retried after a partial
        // failure, the unique constraint returns the original handle instead of
        // taking custody twice.
        let existing: Option<String> =
            sqlx::query_scalar("select handle from ledger_holds where deal_id = $1")
                .bind(deal_id.as_uuid())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?;
        if let Some(handle) = existing {
            return Ok(handle);
        }

        let handle = format!("hold_{}", uuid::Uuid::new_v4().simple());
        let now = OffsetDateTime::now_utc();
        let mut tx = self.pool.begin().await.map_err(storage)?;

        sqlx::query(
            "insert into ledger_holds (handle, deal_id, currency, minor_units, state, created_at) \
             values ($1,$2,$3,$4,'held',$5)",
        )
        .bind(&handle)
        .bind(deal_id.as_uuid())
        .bind(&amount.currency)
        .bind(amount.minor_units)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        for (account, delta) in [
            (format!("buyer:{}", deal_id.as_uuid()), -amount.minor_units),
            ("escrow".to_string(), amount.minor_units),
        ] {
            sqlx::query(
                "insert into ledger_entries (handle, account, currency, minor_units, at, memo) \
                 values ($1,$2,$3,$4,$5,'funds held in escrow')",
            )
            .bind(&handle)
            .bind(&account)
            .bind(&amount.currency)
            .bind(delta)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }

        tx.commit().await.map_err(storage)?;
        info!(deal_id = %deal_id, handle, amount = %amount, "escrow holding funds");
        Ok(handle)
    }

    async fn release(&self, handle: &str) -> Result<SettlementState> {
        self.settle(handle, "released", "seller", "released to seller")
            .await
    }

    async fn refund(&self, handle: &str) -> Result<SettlementState> {
        self.settle(handle, "refunded", "buyer", "refunded to buyer")
            .await
    }

    async fn state(&self, handle: &str) -> Result<SettlementState> {
        self.state_of(handle).await
    }
}
