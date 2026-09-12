//! Runtime-owned SQLite audit, replay, and reporting storage.

use std::path::Path;
use std::str::FromStr;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::TradeDecision;
use crate::execution::{ExecutionPlan, ExecutionReceipt};
use crate::intelligence::IntelligenceEvidence;
use crate::risk::RiskDecision;

const MIGRATION_VERSION: i64 = 1;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("migration checksum/version mismatch")]
    MigrationMismatch,
    #[error("invalid operator projection: {0}")]
    InvalidOperatorProjection(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct RunId(Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuditEvent {
    pub event_id: Uuid,
    pub run_id: RunId,
    pub event_type: String,
    pub entity_id: String,
    pub occurred_at: OffsetDateTime,
    pub payload: Value,
}

impl AuditEvent {
    pub fn new(
        run_id: RunId,
        event_type: impl Into<String>,
        entity_id: impl Into<String>,
        occurred_at: OffsetDateTime,
        payload: Value,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            run_id,
            event_type: event_type.into(),
            entity_id: entity_id.into(),
            occurred_at,
            payload,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplayContext {
    pub events: Vec<AuditEvent>,
}

impl ReplayContext {
    pub fn serialized(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredOperatorProjection {
    pub run_id: RunId,
    pub revision: u64,
    pub updated_at: OffsetDateTime,
    pub payload: Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ReportSummary {
    pub decisions: usize,
    pub holds: usize,
    pub rejected_risk_decisions: usize,
    pub executions: usize,
    pub unknown_executions: usize,
    pub fills: usize,
    pub fees: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FillRecord {
    pub execution_id: Uuid,
    pub fill_id: Uuid,
    pub occurred_at: OffsetDateTime,
    pub quantity: rust_decimal::Decimal,
    pub price: rust_decimal::Decimal,
    pub fee: rust_decimal::Decimal,
    pub payload: Value,
}

pub struct AuditStore {
    connection: Connection,
}

impl AuditStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StorageError> {
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = FULL;
             PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;",
        )?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                checksum TEXT NOT NULL,
                applied_at INTEGER NOT NULL
            );",
        )?;
        let current: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        if current > MIGRATION_VERSION {
            return Err(StorageError::MigrationMismatch);
        }
        if current == MIGRATION_VERSION {
            let checksum: String = self.connection.query_row(
                "SELECT checksum FROM schema_migrations WHERE version = ?1",
                [MIGRATION_VERSION],
                |row| row.get(0),
            )?;
            if checksum != "prooftrade-schema-v1" {
                return Err(StorageError::MigrationMismatch);
            }
            self.ensure_operator_projection_table()?;
            return Ok(());
        }

        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute_batch(
            "CREATE TABLE runs (
                run_id TEXT PRIMARY KEY,
                started_at INTEGER NOT NULL
            );
            CREATE TABLE audit_events (
                event_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL REFERENCES runs(run_id),
                event_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                occurred_at INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX audit_events_entity_idx ON audit_events(entity_id, occurred_at);
            CREATE TABLE evidence (
                evidence_id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                source_event_id TEXT NOT NULL,
                lineage_id TEXT,
                payload TEXT NOT NULL
            );
            CREATE UNIQUE INDEX evidence_source_event_idx ON evidence(source, source_event_id);
            CREATE TABLE decisions (
                decision_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL REFERENCES runs(run_id),
                instrument TEXT NOT NULL,
                action TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE TABLE risk_decisions (
                risk_decision_id TEXT PRIMARY KEY,
                decision_id TEXT NOT NULL REFERENCES decisions(decision_id),
                outcome TEXT NOT NULL,
                evaluated_at INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE TABLE executions (
                execution_id TEXT PRIMARY KEY,
                risk_decision_id TEXT NOT NULL REFERENCES risk_decisions(risk_decision_id),
                client_order_id TEXT NOT NULL UNIQUE,
                exchange_order_id TEXT,
                status TEXT NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE TABLE fills (
                fill_id TEXT PRIMARY KEY,
                execution_id TEXT NOT NULL REFERENCES executions(execution_id),
                quantity TEXT NOT NULL,
                price TEXT NOT NULL,
                fee TEXT NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE TABLE portfolio_snapshots (
                snapshot_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL REFERENCES runs(run_id),
                observed_at INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            INSERT INTO schema_migrations(version, checksum, applied_at)
            VALUES (1, 'prooftrade-schema-v1', strftime('%s', 'now'));",
        )?;
        transaction.commit()?;
        self.ensure_operator_projection_table()?;
        Ok(())
    }

    fn ensure_operator_projection_table(&self) -> Result<(), StorageError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS operator_projections (
                projection_key TEXT PRIMARY KEY,
                run_id TEXT NOT NULL REFERENCES runs(run_id),
                revision INTEGER NOT NULL,
                updated_at TEXT NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS operator_projection_run_idx
                ON operator_projections(run_id, revision);",
        )?;
        Ok(())
    }

    pub fn migration_version(&self) -> Result<i64, StorageError> {
        Ok(self.connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn journal_mode(&self) -> Result<String, StorageError> {
        Ok(self
            .connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))?)
    }

    pub fn append_event(&mut self, event: AuditEvent) -> Result<(), StorageError> {
        let transaction = self.connection.transaction()?;
        ensure_run_tx(
            &transaction,
            event.run_id,
            event.occurred_at.unix_timestamp(),
        )?;
        insert_event(&transaction, event)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn append_event_with_projection(
        &mut self,
        event: AuditEvent,
        revision: u64,
        projection: Value,
    ) -> Result<(), StorageError> {
        let revision = i64::try_from(revision).map_err(|_| {
            StorageError::InvalidOperatorProjection("revision exceeds SQLite INTEGER".to_owned())
        })?;
        let event_payload = sanitize_payload(event.payload.clone());
        let projection = sanitize_payload(projection);
        let projection_json = serde_json::to_string(&projection)?;
        let event_json = serde_json::to_string(&event_payload)?;
        let run_id = event.run_id.as_uuid().to_string();
        let projection_key = format!("run:{run_id}");
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO runs(run_id, started_at) VALUES (?1, ?2)
             ON CONFLICT(run_id) DO NOTHING",
            params![run_id, event.occurred_at.unix_timestamp()],
        )?;
        transaction.execute(
            "INSERT INTO audit_events(event_id, run_id, event_type, entity_id, occurred_at, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.event_id.to_string(),
                event.run_id.as_uuid().to_string(),
                event.event_type,
                event.entity_id,
                event.occurred_at.unix_timestamp_nanos().to_string(),
                event_json,
            ],
        )?;
        for key in ["current".to_owned(), projection_key] {
            transaction.execute(
                "INSERT INTO operator_projections(projection_key, run_id, revision, updated_at, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(projection_key) DO UPDATE SET
                   run_id = excluded.run_id,
                   revision = excluded.revision,
                   updated_at = excluded.updated_at,
                   payload = excluded.payload",
                params![
                    key,
                    event.run_id.as_uuid().to_string(),
                    revision,
                    event.occurred_at.unix_timestamp_nanos().to_string(),
                    projection_json.clone(),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn load_current_operator_projection(
        &self,
    ) -> Result<Option<StoredOperatorProjection>, StorageError> {
        self.load_operator_projection_key("current")
    }

    pub fn load_operator_projection(
        &self,
        run_id: RunId,
    ) -> Result<Option<StoredOperatorProjection>, StorageError> {
        self.load_operator_projection_key(&format!("run:{}", run_id.as_uuid()))
    }

    pub fn load_latest_run(&self) -> Result<Option<RunId>, StorageError> {
        let result = self
            .connection
            .query_row(
                "SELECT run_id FROM runs ORDER BY started_at DESC, run_id DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        result
            .map(|value| {
                Uuid::parse_str(&value)
                    .map(RunId::from_uuid)
                    .map_err(|_| rusqlite::Error::InvalidQuery)
            })
            .transpose()
            .map_err(StorageError::Database)
    }

    fn load_operator_projection_key(
        &self,
        projection_key: &str,
    ) -> Result<Option<StoredOperatorProjection>, StorageError> {
        let result = self
            .connection
            .query_row(
                "SELECT run_id, revision, updated_at, payload
                 FROM operator_projections WHERE projection_key = ?1",
                [projection_key],
                |row| {
                    let run_id: String = row.get(0)?;
                    let revision: i64 = row.get(1)?;
                    let updated_at: String = row.get(2)?;
                    let payload: String = row.get(3)?;
                    Ok((run_id, revision, updated_at, payload))
                },
            )
            .optional()?;
        let Some((run_id, revision, updated_at, payload)) = result else {
            return Ok(None);
        };
        let run_id = Uuid::parse_str(&run_id)
            .map(RunId::from_uuid)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let revision = u64::try_from(revision).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let nanos = updated_at
            .parse::<i128>()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let updated_at = OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let payload = serde_json::from_str(&payload)?;
        Ok(Some(StoredOperatorProjection {
            run_id,
            revision,
            updated_at,
            payload,
        }))
    }

    pub fn save_decision_bundle(
        &mut self,
        run_id: RunId,
        decision: &TradeDecision,
        risk: &RiskDecision,
    ) -> Result<(), StorageError> {
        let decision_payload = serde_json::to_value(decision)?;
        let risk_payload = serde_json::to_value(risk)?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO runs(run_id, started_at) VALUES (?1, ?2)
             ON CONFLICT(run_id) DO NOTHING",
            params![
                run_id.as_uuid().to_string(),
                decision.created_at.unix_timestamp()
            ],
        )?;
        transaction.execute(
            "INSERT INTO decisions(decision_id, run_id, instrument, action, created_at, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                decision.decision_id.as_uuid().to_string(),
                run_id.as_uuid().to_string(),
                decision.instrument.to_string(),
                serde_json::to_string(&decision.action)?,
                decision.created_at.unix_timestamp(),
                serde_json::to_string(&sanitize_payload(decision_payload))?,
            ],
        )?;
        transaction.execute(
            "INSERT INTO risk_decisions(risk_decision_id, decision_id, outcome, evaluated_at, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                risk.risk_decision_id.to_string(),
                decision.decision_id.as_uuid().to_string(),
                serde_json::to_string(&risk.outcome)?,
                risk.evaluated_at.unix_timestamp(),
                serde_json::to_string(&sanitize_payload(risk_payload))?,
            ],
        )?;
        let decision_event = AuditEvent::new(
            run_id,
            "decision",
            decision.decision_id.as_uuid().to_string(),
            decision.created_at,
            serde_json::to_value(decision)?,
        );
        let risk_event = AuditEvent::new(
            run_id,
            "risk_decision",
            decision.decision_id.as_uuid().to_string(),
            risk.evaluated_at,
            serde_json::to_value(risk)?,
        );
        insert_event(&transaction, decision_event)?;
        insert_event(&transaction, risk_event)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_evidence(
        &mut self,
        run_id: RunId,
        evidence: &IntelligenceEvidence,
    ) -> Result<(), StorageError> {
        let payload = sanitize_payload(serde_json::to_value(evidence)?);
        let lineage_id = evidence.lineage_id.map(|value| value.as_uuid().to_string());
        let transaction = self.connection.transaction()?;
        ensure_run_tx(&transaction, run_id, evidence.observed_at.unix_timestamp())?;
        let source = serde_json::to_string(&evidence.source)?;
        let payload_json = serde_json::to_string(&payload)?;
        let updated = transaction.execute(
            "UPDATE evidence SET evidence_id = ?1, lineage_id = ?2, payload = ?3
             WHERE source = ?4 AND source_event_id = ?5",
            params![
                evidence.evidence_id.as_uuid().to_string(),
                lineage_id,
                payload_json,
                source,
                evidence.source_event_id.as_str(),
            ],
        )?;
        if updated == 0 {
            transaction.execute(
                "INSERT INTO evidence(evidence_id, source, source_event_id, lineage_id, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    evidence.evidence_id.as_uuid().to_string(),
                    serde_json::to_string(&evidence.source)?,
                    evidence.source_event_id.as_str(),
                    lineage_id,
                    serde_json::to_string(&payload)?,
                ],
            )?;
        }
        insert_event(
            &transaction,
            AuditEvent::new(
                run_id,
                "evidence",
                evidence.evidence_id.as_uuid().to_string(),
                evidence.observed_at,
                payload,
            ),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn count_evidence(&self) -> Result<usize, StorageError> {
        let count = self
            .connection
            .query_row("SELECT COUNT(*) FROM evidence", [], |row| {
                row.get::<_, i64>(0)
            })?;
        usize::try_from(count).map_err(|_| rusqlite::Error::InvalidQuery.into())
    }

    pub fn save_execution(
        &mut self,
        run_id: RunId,
        plan: &ExecutionPlan,
        receipt: &ExecutionReceipt,
    ) -> Result<(), StorageError> {
        let payload = sanitize_payload(serde_json::json!({"plan": plan, "receipt": receipt}));
        let transaction = self.connection.transaction()?;
        ensure_run_tx(&transaction, run_id, plan.created_at.unix_timestamp())?;
        transaction.execute(
            "INSERT INTO executions(execution_id, risk_decision_id, client_order_id, exchange_order_id, status, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(execution_id) DO UPDATE SET exchange_order_id = excluded.exchange_order_id, status = excluded.status, payload = excluded.payload",
            params![
                plan.execution_id.to_string(),
                plan.risk_decision_id.to_string(),
                plan.client_order_id,
                receipt.exchange_order_id,
                serde_json::to_string(&receipt.state)?,
                serde_json::to_string(&payload)?,
            ],
        )?;
        insert_event(
            &transaction,
            AuditEvent::new(
                run_id,
                "execution",
                plan.decision_id.as_uuid().to_string(),
                plan.created_at,
                serde_json::to_value(receipt)?,
            ),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_fill(&mut self, run_id: RunId, fill: &FillRecord) -> Result<(), StorageError> {
        let payload = sanitize_payload(fill.payload.clone());
        let transaction = self.connection.transaction()?;
        ensure_run_tx(&transaction, run_id, fill.occurred_at.unix_timestamp())?;
        transaction.execute(
            "INSERT INTO fills(fill_id, execution_id, quantity, price, fee, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                fill.fill_id.to_string(),
                fill.execution_id.to_string(),
                fill.quantity.to_string(),
                fill.price.to_string(),
                fill.fee.to_string(),
                serde_json::to_string(&payload)?,
            ],
        )?;
        insert_event(
            &transaction,
            AuditEvent::new(
                run_id,
                "fill",
                fill.execution_id.to_string(),
                fill.occurred_at,
                payload,
            ),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_portfolio_snapshot(
        &mut self,
        run_id: RunId,
        snapshot_id: Uuid,
        observed_at: OffsetDateTime,
        payload: Value,
    ) -> Result<(), StorageError> {
        let payload = sanitize_payload(payload);
        let transaction = self.connection.transaction()?;
        ensure_run_tx(&transaction, run_id, observed_at.unix_timestamp())?;
        transaction.execute(
            "INSERT INTO portfolio_snapshots(snapshot_id, run_id, observed_at, payload)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                snapshot_id.to_string(),
                run_id.as_uuid().to_string(),
                observed_at.unix_timestamp(),
                serde_json::to_string(&payload)?,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn replay(&self, entity_id: Uuid) -> Result<ReplayContext, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT event_id, run_id, event_type, entity_id, occurred_at, payload
             FROM audit_events WHERE entity_id = ?1 ORDER BY occurred_at, event_id",
        )?;
        let rows = statement.query_map([entity_id.to_string()], |row| {
            let event_id: String = row.get(0)?;
            let run_id: String = row.get(1)?;
            let occurred_at: i64 = row.get(4)?;
            let payload: String = row.get(5)?;
            Ok((
                event_id,
                run_id,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                occurred_at,
                payload,
            ))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (event_id, run_id, event_type, entity_id, occurred_at, payload) = row?;
            let event_id = Uuid::parse_str(&event_id).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let run_id = Uuid::parse_str(&run_id).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let nanos = i128::from(occurred_at);
            let occurred_at = OffsetDateTime::from_unix_timestamp_nanos(nanos)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            events.push(AuditEvent {
                event_id,
                run_id: RunId::from_uuid(run_id),
                event_type,
                entity_id,
                occurred_at,
                payload: serde_json::from_str(&payload)?,
            });
        }
        Ok(ReplayContext { events })
    }

    pub fn count_events_for(&self, entity_id: Uuid) -> Result<usize, StorageError> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM audit_events WHERE entity_id = ?1",
            [entity_id.to_string()],
            |row| row.get::<_, i64>(0),
        )? as usize)
    }

    pub fn count_events_for_run(&self, run_id: RunId) -> Result<usize, StorageError> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM audit_events WHERE run_id = ?1",
            [run_id.as_uuid().to_string()],
            |row| row.get::<_, i64>(0),
        )? as usize)
    }

    pub fn unresolved_execution_ids(&self) -> Result<Vec<Uuid>, StorageError> {
        let mut statement = self.connection.prepare(
            "WITH latest AS (
                SELECT
                    COALESCE(json_extract(payload, '$.execution_id'), entity_id) AS execution_id,
                    json_extract(payload, '$.state') AS state,
                    json_extract(payload, '$.status') AS status,
                    ROW_NUMBER() OVER (
                        PARTITION BY COALESCE(json_extract(payload, '$.execution_id'), entity_id)
                        ORDER BY occurred_at DESC, event_id DESC
                    ) AS row_number
                FROM audit_events
                WHERE event_type = 'execution'
            )
            SELECT execution_id FROM latest
            WHERE row_number = 1 AND (state = 'Unknown' OR status = 'Unknown')",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            if let Ok(id) = Uuid::parse_str(&row?) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    pub fn report(&self) -> Result<ReportSummary, StorageError> {
        let count = |sql: &str| -> Result<usize, StorageError> {
            Ok(self
                .connection
                .query_row(sql, [], |row| row.get::<_, i64>(0))? as usize)
        };
        let mut fees = rust_decimal::Decimal::ZERO;
        let mut fee_rows = self
            .connection
            .prepare("SELECT payload FROM audit_events WHERE event_type = 'fill'")?;
        for row in fee_rows.query_map([], |row| row.get::<_, String>(0))? {
            let payload: Value = serde_json::from_str(&row?)?;
            if let Some(value) = payload.get("fee").and_then(Value::as_str)
                && let Ok(value) = rust_decimal::Decimal::from_str(value)
            {
                fees += value;
            }
        }
        Ok(ReportSummary {
            decisions: count("SELECT COUNT(*) FROM audit_events WHERE event_type = 'decision'")?,
            holds: count("SELECT COUNT(*) FROM decisions WHERE action = '\"HOLD\"'")?,
            rejected_risk_decisions: count(
                "SELECT COUNT(*) FROM risk_decisions WHERE outcome = '\"Rejected\"'",
            )?,
            executions: count("SELECT COUNT(*) FROM audit_events WHERE event_type = 'execution'")?,
            unknown_executions: count(
                "WITH latest AS (
                    SELECT
                        json_extract(payload, '$.state') AS state,
                        json_extract(payload, '$.status') AS status,
                        ROW_NUMBER() OVER (
                            PARTITION BY COALESCE(json_extract(payload, '$.execution_id'), entity_id)
                            ORDER BY occurred_at DESC, event_id DESC
                        ) AS row_number
                    FROM audit_events
                    WHERE event_type = 'execution'
                )
                SELECT COUNT(*) FROM latest
                WHERE row_number = 1 AND (state = 'Unknown' OR status = 'Unknown')",
            )?,
            fills: count("SELECT COUNT(*) FROM audit_events WHERE event_type = 'fill'")?,
            fees,
        })
    }
}

fn ensure_run_tx(
    transaction: &rusqlite::Transaction<'_>,
    run_id: RunId,
    started_at: i64,
) -> Result<(), StorageError> {
    transaction.execute(
        "INSERT INTO runs(run_id, started_at) VALUES (?1, ?2) ON CONFLICT(run_id) DO NOTHING",
        params![run_id.as_uuid().to_string(), started_at],
    )?;
    Ok(())
}

fn insert_event(
    transaction: &rusqlite::Transaction<'_>,
    event: AuditEvent,
) -> Result<(), StorageError> {
    transaction.execute(
        "INSERT INTO audit_events(event_id, run_id, event_type, entity_id, occurred_at, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event.event_id.to_string(),
            event.run_id.as_uuid().to_string(),
            event.event_type,
            event.entity_id,
            event.occurred_at.unix_timestamp_nanos().to_string(),
            serde_json::to_string(&sanitize_payload(event.payload))?,
        ],
    )?;
    Ok(())
}

fn sanitize_payload(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut sanitized = Map::new();
            for (key, value) in object {
                let normalized: String = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(|character| character.to_lowercase())
                    .collect();
                if matches!(
                    normalized.as_str(),
                    "apikey"
                        | "apisecret"
                        | "secret"
                        | "passphrase"
                        | "authorization"
                        | "accesstoken"
                        | "password"
                        | "privatekey"
                        | "bearertoken"
                ) {
                    continue;
                }
                sanitized.insert(key, sanitize_payload(value));
            }
            Value::Object(sanitized)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(sanitize_payload).collect()),
        other => other,
    }
}
