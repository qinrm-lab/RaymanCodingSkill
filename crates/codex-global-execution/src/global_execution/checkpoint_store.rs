//! Tenant-scoped row changes for checkpoint databases. SQL text is fixed by
//! this module; callers supply typed values and expected previous-row hashes.
use super::*;
use rusqlite::{
    Connection, OptionalExtension, params_from_iter,
    types::{Value, ValueRef},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cell {
    Null,
    Integer { value: i64 },
    Text { value: String },
    Blob { sha256: String, size: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowChange {
    pub table: String,
    pub key: Vec<String>,
    pub before_sha256: Option<String>,
    pub after: Option<Vec<Cell>>,
}

// Schema v3 from SaveStatus vault.rs; backend databases are created from this
// fixed schema. Legacy layouts require verified logical migration first.
const SCHEMA_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS workspaces(
 workspace_id TEXT PRIMARY KEY,canonical_path TEXT NOT NULL UNIQUE,
 active INTEGER NOT NULL CHECK(active IN (0,1)),source_skill_dir TEXT,
 interval_minutes INTEGER NOT NULL,config_json TEXT NOT NULL,
 installed_at_utc TEXT NOT NULL,updated_at_utc TEXT NOT NULL,uninstalled_at_utc TEXT
);
CREATE TABLE IF NOT EXISTS sessions(
 workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 session_id TEXT NOT NULL,started_at_utc TEXT NOT NULL,interval_seconds INTEGER NOT NULL,
 next_due_at_utc TEXT NOT NULL,last_checkpoint_id TEXT,last_checkpoint_at_utc TEXT,
 save_count INTEGER NOT NULL,schedule_git_head TEXT,git_cadence_reset_at_utc TEXT,
 active_seconds INTEGER NOT NULL DEFAULT 0,activity_started_at_utc TEXT,
 last_activity_at_utc TEXT,last_activity_event TEXT,last_recovery_json TEXT,blocker_json TEXT
);
CREATE TABLE IF NOT EXISTS checkpoints(
 workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 checkpoint_id TEXT NOT NULL,session_id TEXT NOT NULL,created_at_utc TEXT NOT NULL,
 agent TEXT NOT NULL,checkpoint_json TEXT NOT NULL,patch_blob BLOB NOT NULL,
 archive_name TEXT NOT NULL,archive_blob BLOB NOT NULL,seal_sha256 TEXT NOT NULL,
 PRIMARY KEY(workspace_id,checkpoint_id)
);
CREATE TABLE IF NOT EXISTS auto_events(
 workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 agent TEXT NOT NULL,event TEXT NOT NULL,runtime_fingerprint TEXT NOT NULL,
 handled_at_utc TEXT NOT NULL,PRIMARY KEY(workspace_id,agent,event)
);
CREATE TABLE IF NOT EXISTS mirror_intents(
 workspace_id TEXT NOT NULL,checkpoint_id TEXT NOT NULL,seal_sha256 TEXT NOT NULL,
 created_at_utc TEXT NOT NULL,PRIMARY KEY(workspace_id,checkpoint_id)
);
CREATE TABLE IF NOT EXISTS session_intents(
 workspace_id TEXT PRIMARY KEY,session_json TEXT NOT NULL,created_at_utc TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_locks(
 workspace_id TEXT PRIMARY KEY,holder TEXT NOT NULL,operation TEXT NOT NULL,
 acquired_at_utc TEXT NOT NULL,heartbeat_at_utc TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS checkpoints_latest
 ON checkpoints(workspace_id,created_at_utc DESC,checkpoint_id DESC);
"#;

pub fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(SCHEMA_DDL)?;
    connection.execute(
        "INSERT INTO schema_meta(key,value) VALUES('schema_version','3')",
        [],
    )?;
    Ok(())
}

const BROKER_DDL: &str = "CREATE TABLE global_requests(workspace_id TEXT NOT NULL,request_id TEXT NOT NULL,request_sha256 TEXT NOT NULL,PRIMARY KEY(workspace_id,request_id))";

pub fn initialize_owned(connection: &Connection) -> Result<()> {
    initialize(connection)?;
    connection.execute_batch(BROKER_DDL)?;
    Ok(())
}

const TABLES: &[(&str, usize, &[&str])] = &[
    (
        "workspaces",
        1,
        &[
            "workspace_id",
            "canonical_path",
            "active",
            "source_skill_dir",
            "interval_minutes",
            "config_json",
            "installed_at_utc",
            "updated_at_utc",
            "uninstalled_at_utc",
        ],
    ),
    (
        "sessions",
        1,
        &[
            "workspace_id",
            "session_id",
            "started_at_utc",
            "interval_seconds",
            "next_due_at_utc",
            "last_checkpoint_id",
            "last_checkpoint_at_utc",
            "save_count",
            "schedule_git_head",
            "git_cadence_reset_at_utc",
            "active_seconds",
            "activity_started_at_utc",
            "last_activity_at_utc",
            "last_activity_event",
            "last_recovery_json",
            "blocker_json",
        ],
    ),
    (
        "checkpoints",
        2,
        &[
            "workspace_id",
            "checkpoint_id",
            "session_id",
            "created_at_utc",
            "agent",
            "checkpoint_json",
            "patch_blob",
            "archive_name",
            "archive_blob",
            "seal_sha256",
        ],
    ),
    (
        "auto_events",
        3,
        &[
            "workspace_id",
            "agent",
            "event",
            "runtime_fingerprint",
            "handled_at_utc",
        ],
    ),
    (
        "mirror_intents",
        2,
        &[
            "workspace_id",
            "checkpoint_id",
            "seal_sha256",
            "created_at_utc",
        ],
    ),
    (
        "session_intents",
        1,
        &["workspace_id", "session_json", "created_at_utc"],
    ),
    (
        "workspace_locks",
        1,
        &[
            "workspace_id",
            "holder",
            "operation",
            "acquired_at_utc",
            "heartbeat_at_utc",
        ],
    ),
];

fn table(name: &str) -> Result<(usize, &'static [&'static str])> {
    TABLES
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, key, columns)| (*key, *columns))
        .ok_or_else(|| anyhow::anyhow!("checkpoint table is not allowed"))
}
fn cells(row: &rusqlite::Row<'_>, count: usize) -> rusqlite::Result<Vec<Cell>> {
    (0..count)
        .map(|i| {
            Ok(match row.get_ref(i)? {
                ValueRef::Null => Cell::Null,
                ValueRef::Integer(value) => Cell::Integer { value },
                ValueRef::Text(bytes) => Cell::Text {
                    value: std::str::from_utf8(bytes)
                        .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                i,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?
                        .into(),
                },
                ValueRef::Blob(bytes) => Cell::Blob {
                    sha256: crate::hash::sha256_bytes(bytes),
                    size: bytes.len() as u64,
                },
                ValueRef::Real(_) => {
                    return Err(rusqlite::Error::InvalidColumnType(
                        i,
                        "checkpoint cell".into(),
                        rusqlite::types::Type::Real,
                    ));
                }
            })
        })
        .collect()
}
pub fn row_digest(row: &[Cell]) -> Result<String> {
    Ok(crate::hash::sha256_bytes(&serde_json::to_vec(row)?))
}

pub fn snapshot(
    connection: &Connection,
    workspace_id: &str,
) -> Result<BTreeMap<String, Vec<Vec<Cell>>>> {
    if !is_id(workspace_id) || !connection.is_autocommit() {
        bail!("checkpoint snapshot requires a workspace and its own transaction");
    }
    let transaction = connection.unchecked_transaction()?;
    validate_schema(&transaction)?;
    let result = snapshot_in_transaction(&transaction, workspace_id)?;
    transaction.commit()?;
    Ok(result)
}

fn snapshot_in_transaction(
    connection: &Connection,
    workspace_id: &str,
) -> Result<BTreeMap<String, Vec<Vec<Cell>>>> {
    let mut snapshot = BTreeMap::new();
    for (name, keys, columns) in TABLES {
        let sql = format!(
            "SELECT {} FROM {name} WHERE workspace_id=?1 ORDER BY {}",
            columns.join(","),
            columns[..*keys].join(",")
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement
            .query_map([workspace_id], |row| cells(row, columns.len()))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        snapshot.insert((*name).into(), rows);
    }
    Ok(snapshot)
}

pub fn diff(
    before: &BTreeMap<String, Vec<Vec<Cell>>>,
    after: &BTreeMap<String, Vec<Vec<Cell>>>,
) -> Result<Vec<RowChange>> {
    for snapshot in [before, after] {
        if snapshot.keys().map(String::as_str).collect::<BTreeSet<_>>()
            != TABLES
                .iter()
                .map(|(name, _, _)| *name)
                .collect::<BTreeSet<_>>()
        {
            bail!("checkpoint snapshot table set differs");
        }
    }
    let mut changes = Vec::new();
    for (name, keys, columns) in TABLES {
        let index = |rows: Option<&Vec<Vec<Cell>>>| -> Result<BTreeMap<Vec<String>, Vec<Cell>>> {
            let mut index = BTreeMap::new();
            for row in rows.into_iter().flatten() {
                if row.len() != columns.len() {
                    bail!("checkpoint snapshot row shape differs");
                }
                let key = row
                    .iter()
                    .take(*keys)
                    .map(|cell| match cell {
                        Cell::Text { value } => Ok(value.clone()),
                        _ => bail!("checkpoint key must be text"),
                    })
                    .collect::<Result<Vec<_>>>()?;
                if index.insert(key, row.clone()).is_some() {
                    bail!("duplicate checkpoint row key");
                }
            }
            Ok(index)
        };
        let old = index(before.get(*name))?;
        let new = index(after.get(*name))?;
        let keys = old
            .keys()
            .chain(new.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for key in keys {
            if old.get(&key) == new.get(&key) {
                continue;
            }
            changes.push(RowChange {
                table: (*name).into(),
                key: key.clone(),
                before_sha256: old.get(&key).map(|row| row_digest(row)).transpose()?,
                after: new.get(&key).cloned(),
            });
        }
    }
    Ok(changes)
}

/// Normalize a legacy schema-v3 vault by copying typed cells into fresh fixed
/// tables. Source SQL is never executed at the destination. The caller owns
/// namespace pinning, backups, routing publication and crash recovery.
pub fn normalize_owned(source: &Connection, destination: &mut Connection) -> Result<String> {
    normalize_database(source, destination, true)
}
pub fn normalize_local(source: &Connection, destination: &mut Connection) -> Result<String> {
    validate_schema(source)?;
    normalize_database(source, destination, false)
}
fn normalize_database(
    source: &Connection,
    destination: &mut Connection,
    owned: bool,
) -> Result<String> {
    if destination.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, u64>(0)
    })? != 0
    {
        bail!("checkpoint migration destination is not empty");
    }
    let read = source.unchecked_transaction()?;
    if read.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))? != "ok" {
        bail!("legacy checkpoint database integrity check failed");
    }

    let objects = read
        .prepare("SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (kind, name, sql) in objects {
        if !matches!(kind.as_str(), "table" | "index")
            || (kind == "table"
                && name != "schema_meta"
                && table(&name).is_err()
                && !(name == "global_requests" && !owned))
            || sql.is_some_and(|sql| sql.to_ascii_uppercase().contains("CREATE VIRTUAL TABLE"))
        {
            bail!("legacy checkpoint schema contains an unsupported object");
        }
    }
    let metadata = read
        .prepare("SELECT key,value FROM schema_meta ORDER BY key")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if metadata != vec![("schema_version".into(), "3".into())] {
        bail!("legacy checkpoint schema must first be upgraded by its application");
    }
    destination.pragma_update(None, "foreign_keys", true)?;
    destination.pragma_update(None, "journal_mode", "DELETE")?;
    destination.pragma_update(None, "synchronous", "FULL")?;
    let write = destination.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if owned {
        initialize_owned(&write)?;
    } else {
        initialize(&write)?;
    }
    let mut proof = BTreeMap::new();
    for (name, keys, columns) in TABLES {
        // Names/order can differ after ALTER TABLE. Types, generated-column
        // absence and the complete column set must still match the application.
        let shape = |connection: &Connection| -> Result<BTreeMap<String, (String, i64)>> {
            Ok(connection
                .prepare(&format!("PRAGMA table_xinfo({name})"))?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(1)?,
                        (row.get::<_, String>(2)?, row.get::<_, i64>(6)?),
                    ))
                })?
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
        };
        if shape(&read)? != shape(&write)? {
            bail!("legacy checkpoint columns differ: {name}");
        }
        let query = format!("SELECT {} FROM {name}", columns.join(","));
        let mut statement = read.prepare(&query)?;
        let mut rows = statement.query([])?;
        let placeholders = (1..=columns.len())
            .map(|n| format!("?{n}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut insert = write.prepare(&format!(
            "INSERT INTO {name} ({}) VALUES ({placeholders})",
            columns.join(",")
        ))?;
        let mut expected = Vec::new();
        while let Some(row) = rows.next()? {
            let typed = cells(row, columns.len())?;
            if typed
                .iter()
                .take(*keys)
                .any(|cell| !matches!(cell,Cell::Text{value} if !value.is_empty()))
                || !matches!(typed.first(),Some(Cell::Text{value}) if is_id(value))
            {
                bail!("legacy checkpoint row key is invalid");
            }
            expected.push(row_digest(&typed)?);
            let values = (0..columns.len())
                .map(|i| -> Result<Value> {
                    Ok(match row.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => Value::Integer(n),
                        ValueRef::Text(bytes) => Value::Text(std::str::from_utf8(bytes)?.into()),
                        ValueRef::Blob(bytes) => Value::Blob(bytes.into()),
                        ValueRef::Real(_) => bail!("legacy real cell is not supported"),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            insert.execute(params_from_iter(values))?;
        }
        let mut observed = write
            .prepare(&query)?
            .query_map([], |row| cells(row, columns.len()))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .map(|row| row_digest(row))
            .collect::<Result<Vec<_>>>()?;
        expected.sort();
        observed.sort();
        if expected != observed {
            bail!("legacy checkpoint row/blob migration differs");
        }
        proof.insert(*name, expected);
    }
    if write.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
        row.get::<_, u64>(0)
    })? != 0
    {
        bail!("migrated checkpoint foreign keys differ");
    }
    read.commit()?;
    write.commit()?;
    validate_schema(destination)?;
    Ok(crate::hash::sha256_bytes(&serde_json::to_vec(&proof)?))
}

/// Working-copy operations may change only their enrolled tenant. Even an
/// ignored foreign row would lose the caller's intended work if accepted.
pub fn other_rows_digest(connection: &Connection, workspace_id: &str) -> Result<String> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema(&transaction)?;
    let mut rows = BTreeMap::new();
    for (name, keys, columns) in TABLES {
        let sql = format!(
            "SELECT {} FROM {name} WHERE workspace_id IS NOT ?1 ORDER BY {}",
            columns.join(","),
            columns[..*keys].join(",")
        );
        let data = transaction
            .prepare(&sql)?
            .query_map([workspace_id], |row| cells(row, columns.len()))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.insert((*name).to_string(), data);
    }
    if transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='global_requests')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        let receipts = transaction.prepare("SELECT workspace_id,request_id,request_sha256 FROM global_requests ORDER BY workspace_id,request_id")?.query_map([],|row|cells(row,3))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.insert("global_requests".into(), receipts);
    }
    transaction.commit()?;
    Ok(crate::hash::sha256_bytes(&serde_json::to_vec(&rows)?))
}

/// Collect only the content-addressed blobs referenced by the computed delta.
/// Names come from the fixed schema; all key values remain SQL parameters.
pub fn changed_blobs(
    connection: &Connection,
    changes: &[RowChange],
) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut blobs = BTreeMap::new();
    let transaction = connection.unchecked_transaction()?;
    validate_schema(&transaction)?;
    for change in changes {
        let (keys, columns) = table(&change.table)?;
        if change.key.len() != keys {
            bail!("checkpoint blob key shape differs");
        }
        let where_clause = columns[..keys]
            .iter()
            .enumerate()
            .map(|(i, name)| format!("{name}=?{}", i + 1))
            .collect::<Vec<_>>()
            .join(" AND ");
        for (index, cell) in change.after.iter().flatten().enumerate() {
            if let Cell::Blob { sha256, size } = cell {
                if index >= columns.len() || *size > 512 * 1024 * 1024 {
                    bail!("checkpoint blob shape or length differs");
                }
                if blobs.contains_key(sha256) {
                    continue;
                }
                let bytes: Vec<u8> = transaction.query_row(
                    &format!(
                        "SELECT {} FROM {} WHERE {where_clause}",
                        columns[index], change.table
                    ),
                    params_from_iter(change.key.iter()),
                    |row| row.get(0),
                )?;
                if bytes.len() as u64 != *size || crate::hash::sha256_bytes(&bytes) != *sha256 {
                    bail!("checkpoint working copy blob changed");
                }
                blobs.insert(sha256.clone(), bytes);
            }
        }
    }
    transaction.commit()?;
    Ok(blobs)
}

/// Compare the entire executable schema, including conflict resolution,
/// collations, CHECK expressions, foreign keys and indexes. Matching column
/// names alone cannot exclude a cross-tenant REPLACE or CASCADE side effect.
pub fn validate_schema(connection: &Connection) -> Result<()> {
    fn definitions(connection: &Connection) -> Result<Vec<(String, String, Option<String>)>> {
        Ok(connection
            .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    let reference = Connection::open_in_memory()?;
    initialize(&reference)?;
    if connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='global_requests')",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        reference.execute_batch(BROKER_DDL)?;
    }
    if definitions(connection)? != definitions(&reference)? {
        bail!("checkpoint database schema differs from the fixed schema v3");
    }
    let metadata = connection
        .prepare("SELECT key,value FROM schema_meta ORDER BY key")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if metadata != vec![("schema_version".into(), "3".into())] {
        bail!("checkpoint schema metadata differs");
    }
    Ok(())
}

pub fn apply(
    connection: &mut Connection,
    workspace_id: &str,
    changes: &[RowChange],
    blobs: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    apply_internal(connection, workspace_id, changes, blobs, None).map(|_| ())
}

/// The receipt is committed in the same SQLite transaction as the rows. A
/// process death after commit but before queue result publication is recoverable.
pub fn apply_once(
    connection: &mut Connection,
    workspace_id: &str,
    changes: &[RowChange],
    blobs: &BTreeMap<String, Vec<u8>>,
    request_id: &str,
    request_sha256: &str,
) -> Result<bool> {
    if !is_id(request_id) || !is_sha256(request_sha256) {
        bail!("invalid checkpoint transaction identity");
    }
    apply_internal(
        connection,
        workspace_id,
        changes,
        blobs,
        Some((request_id, request_sha256)),
    )
}

fn apply_internal(
    connection: &mut Connection,
    workspace_id: &str,
    changes: &[RowChange],
    blobs: &BTreeMap<String, Vec<u8>>,
    receipt: Option<(&str, &str)>,
) -> Result<bool> {
    if !is_id(workspace_id) || changes.len() > 10000 {
        bail!("invalid checkpoint row transaction");
    }
    connection.pragma_update(None, "trusted_schema", false)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    if !connection.is_autocommit() {
        bail!("checkpoint backend requires its own transaction");
    }
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // Validate while holding SQLite's writer lock, before executing any supplied data.
    validate_schema(&transaction)?;
    if let Some((id, sha)) = receipt {
        let previous = transaction.query_row("SELECT request_sha256 FROM global_requests WHERE workspace_id=?1 AND request_id=?2", [workspace_id, id], |row| row.get::<_, String>(0)).optional()?;
        if let Some(previous) = previous {
            if previous != sha {
                bail!("checkpoint request ID reused with different data");
            }
            transaction.commit()?;
            return Ok(true);
        }
    }
    let before = snapshot_in_transaction(&transaction, workspace_id)?;
    let mut seen = BTreeSet::new();
    for change in changes {
        if !seen.insert((&change.table, &change.key)) {
            bail!("duplicate checkpoint row mutation");
        }
        if change.table == "workspaces" && change.after.is_none() {
            // An ordinary row update must never authorize implicit CASCADE deletes.
            bail!("workspace removal requires the separately verified migration route");
        }
    }
    transaction.pragma_update(None, "defer_foreign_keys", true)?;
    for change in changes {
        let (keys, columns) = table(&change.table)?;
        if change.key.len() != keys || change.key.first().map(String::as_str) != Some(workspace_id)
        {
            bail!("checkpoint mutation crosses workspace boundary");
        }
        let where_clause = columns[..keys]
            .iter()
            .enumerate()
            .map(|(i, name)| format!("{name}=?{}", i + 1))
            .collect::<Vec<_>>()
            .join(" AND ");
        let current = transaction
            .query_row(
                &format!(
                    "SELECT {} FROM {} WHERE {where_clause}",
                    columns.join(","),
                    change.table
                ),
                params_from_iter(change.key.iter()),
                |row| cells(row, columns.len()),
            )
            .optional()?;
        if current.as_ref().map(|row| row_digest(row)).transpose()? != change.before_sha256 {
            bail!("checkpoint row compare-and-swap conflict");
        }
        if let Some(after) = &change.after {
            if after.len() != columns.len()
                || after
                    .iter()
                    .take(keys)
                    .zip(&change.key)
                    .any(|(cell, key)| !matches!(cell,Cell::Text{value} if value==key))
            {
                bail!("checkpoint row shape or key changed");
            }
            let values = after
                .iter()
                .map(|cell| -> Result<Value> {
                    Ok(match cell {
                        Cell::Null => Value::Null,
                        Cell::Integer { value } => Value::Integer(*value),
                        Cell::Text { value } => Value::Text(value.clone()),
                        Cell::Blob { sha256, size } => {
                            let bytes = blobs
                                .get(sha256)
                                .ok_or_else(|| anyhow::anyhow!("checkpoint blob missing"))?;
                            if bytes.len() as u64 != *size
                                || crate::hash::sha256_bytes(bytes) != *sha256
                            {
                                bail!("checkpoint blob identity mismatch");
                            }
                            Value::Blob(bytes.clone())
                        }
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let placeholders = (1..=columns.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let update = columns[keys..]
                .iter()
                .map(|name| format!("{name}=excluded.{name}"))
                .collect::<Vec<_>>()
                .join(",");
            transaction.execute(&format!("INSERT INTO {} ({}) VALUES ({placeholders}) ON CONFLICT ({}) DO UPDATE SET {update}",change.table,columns.join(","),columns[..keys].join(",")),params_from_iter(values))?;
        } else {
            transaction.execute(
                &format!("DELETE FROM {} WHERE {where_clause}", change.table),
                params_from_iter(change.key.iter()),
            )?;
        }
    }
    let after = snapshot_in_transaction(&transaction, workspace_id)?;
    let actual = diff(&before, &after)?;
    let expected = serde_json::to_value(changes)?;
    let observed = serde_json::to_value(&actual)?;
    // Compare keyed changes, independent of request order. This detects any
    // unintended same-tenant effects in addition to schema-enforced isolation.
    let ordered = |value: serde_json::Value| -> Result<BTreeMap<String, serde_json::Value>> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                Ok((
                    serde_json::to_string(&(&row["table"], &row["key"]))?,
                    row.clone(),
                ))
            })
            .collect()
    };
    if ordered(expected)? != ordered(observed)? {
        bail!("checkpoint transaction effects differ from requested rows");
    }
    if let Some((id, sha)) = receipt {
        transaction.execute(
            "INSERT INTO global_requests(workspace_id,request_id,request_sha256) VALUES(?1,?2,?3)",
            [workspace_id, id, sha],
        )?;
    }
    transaction.commit()?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_row_updates_are_scoped_atomic_and_conflict_checked() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let id = "a".repeat(32);
        let other = "b".repeat(32);
        let row = vec![
            Cell::Text { value: id.clone() },
            Cell::Text { value: "{}".into() },
            Cell::Text {
                value: "now".into(),
            },
        ];
        let change = RowChange {
            table: "session_intents".into(),
            key: vec![id.clone()],
            before_sha256: None,
            after: Some(row.clone()),
        };
        apply(
            &mut connection,
            &id,
            std::slice::from_ref(&change),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(
            apply(
                &mut connection,
                &id,
                std::slice::from_ref(&change),
                &BTreeMap::new()
            )
            .is_err()
        );
        let mut hostile = change.clone();
        hostile.key = vec![other];
        assert!(apply(&mut connection, &id, &[hostile], &BTreeMap::new()).is_err());
        let remove = RowChange {
            table: "session_intents".into(),
            key: vec![id.clone()],
            before_sha256: Some(row_digest(&row).unwrap()),
            after: None,
        };
        let mut bad = change;
        bad.table = "sqlite_master".into();
        assert!(apply(&mut connection, &id, &[remove, bad], &BTreeMap::new()).is_err());
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM session_intents", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn checkpoint_row_backend_rejects_trigger_side_effects() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection.execute_batch("CREATE TRIGGER mutate_others AFTER INSERT ON session_intents BEGIN DELETE FROM session_intents WHERE workspace_id!=new.workspace_id; END;").unwrap();
        let id = "a".repeat(32);
        let change = RowChange {
            table: "session_intents".into(),
            key: vec![id.clone()],
            before_sha256: None,
            after: Some(vec![
                Cell::Text { value: id.clone() },
                Cell::Text { value: "{}".into() },
                Cell::Text {
                    value: "now".into(),
                },
            ]),
        };
        assert!(apply(&mut connection, &id, &[change], &BTreeMap::new()).is_err());
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM session_intents", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn checkpoint_backend_rejects_cross_tenant_replace_and_foreign_keys() {
        for hostile in [
            SCHEMA_DDL.replace("session_json TEXT NOT NULL", "session_json TEXT NOT NULL UNIQUE ON CONFLICT REPLACE"),
            SCHEMA_DDL.replace("session_json TEXT NOT NULL", "session_json TEXT NOT NULL REFERENCES session_intents(workspace_id) ON DELETE CASCADE"),
            SCHEMA_DDL.replace("CREATE INDEX IF NOT EXISTS checkpoints_latest", "CREATE UNIQUE INDEX IF NOT EXISTS checkpoints_latest"),
        ] {
            let mut connection = Connection::open_in_memory().unwrap();
            connection.pragma_update(None, "foreign_keys", false).unwrap();
            connection.execute_batch(&hostile).unwrap();
            connection.execute("INSERT INTO schema_meta VALUES('schema_version','3')", []).unwrap();
            connection.execute("INSERT INTO session_intents VALUES(?1,'shared','now')", ["b".repeat(32)]).unwrap();
            let change = RowChange {
                table: "session_intents".into(), key: vec!["a".repeat(32)], before_sha256: None,
                after: Some(vec![Cell::Text{value:"a".repeat(32)}, Cell::Text{value:"shared".into()}, Cell::Text{value:"now".into()}]),
            };
            assert!(apply(&mut connection, &"a".repeat(32), &[change], &BTreeMap::new()).is_err());
            assert_eq!(connection.query_row("SELECT workspace_id FROM session_intents", [], |r| r.get::<_, String>(0)).unwrap(), "b".repeat(32));
        }
    }

    #[test]
    fn checkpoint_backend_preserves_other_tenants_and_rejects_duplicate_rows() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let id = "a".repeat(32);
        let other = "b".repeat(32);
        connection
            .execute(
                "INSERT INTO session_intents VALUES(?1,'shared','before')",
                [&other],
            )
            .unwrap();
        let other_before = snapshot(&connection, &other).unwrap();
        let before = snapshot(&connection, &id).unwrap();
        let mut after = before.clone();
        after.get_mut("session_intents").unwrap().push(vec![
            Cell::Text { value: id.clone() },
            Cell::Text {
                value: "shared".into(),
            },
            Cell::Text {
                value: "after".into(),
            },
        ]);
        let changes = diff(&before, &after).unwrap();
        assert!(
            apply(
                &mut connection,
                &id,
                &[changes[0].clone(), changes[0].clone()],
                &BTreeMap::new()
            )
            .is_err()
        );
        assert_eq!(snapshot(&connection, &id).unwrap(), before);
        apply(&mut connection, &id, &changes, &BTreeMap::new()).unwrap();
        assert_eq!(snapshot(&connection, &id).unwrap(), after);
        assert_eq!(snapshot(&connection, &other).unwrap(), other_before);
        let mut remove_workspace = changes[0].clone();
        remove_workspace.table = "workspaces".into();
        remove_workspace.after = None;
        assert!(apply(&mut connection, &id, &[remove_workspace], &BTreeMap::new()).is_err());
        let mut malformed = before.clone();
        malformed.remove("sessions");
        assert!(diff(&malformed, &after).is_err());
    }

    #[test]
    fn checkpoint_schema_matches_savestatus_v3_fixture_and_cas_rolls_back() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let id = "a".repeat(32);
        let before = snapshot(&connection, &id).unwrap();
        let mut after = before.clone();
        after.get_mut("session_intents").unwrap().push(vec![
            Cell::Text { value: id.clone() },
            Cell::Text { value: "{}".into() },
            Cell::Text {
                value: "now".into(),
            },
        ]);
        after.get_mut("mirror_intents").unwrap().push(vec![
            Cell::Text { value: id.clone() },
            Cell::Text {
                value: "checkpoint".into(),
            },
            Cell::Text {
                value: "c".repeat(64),
            },
            Cell::Text {
                value: "now".into(),
            },
        ]);
        let mut changes = diff(&before, &after).unwrap();
        changes.last_mut().unwrap().before_sha256 = Some("f".repeat(64));
        assert!(
            apply(&mut connection, &id, &changes, &BTreeMap::new())
                .unwrap_err()
                .to_string()
                .contains("compare-and-swap")
        );
        assert_eq!(snapshot(&connection, &id).unwrap(), before);
    }
    #[test]
    fn checkpoint_receipt_and_rows_survive_reopen_without_replaying_writes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("owned.sqlite3");
        let mut connection = Connection::open(&path).unwrap();
        initialize_owned(&connection).unwrap();
        let id = "a".repeat(32);
        let before = snapshot(&connection, &id).unwrap();
        let mut after = before.clone();
        after.get_mut("session_intents").unwrap().push(vec![
            Cell::Text { value: id.clone() },
            Cell::Text { value: "{}".into() },
            Cell::Text {
                value: "now".into(),
            },
        ]);
        let changes = diff(&before, &after).unwrap();
        let request = "d".repeat(32);
        let sha = crate::hash::sha256_bytes(&serde_json::to_vec(&changes).unwrap());
        assert!(
            !apply_once(
                &mut connection,
                &id,
                &changes,
                &BTreeMap::new(),
                &request,
                &sha
            )
            .unwrap()
        );
        drop(connection);
        let mut connection = Connection::open(&path).unwrap();
        assert!(
            apply_once(
                &mut connection,
                &id,
                &changes,
                &BTreeMap::new(),
                &request,
                &sha
            )
            .unwrap()
        );
        assert_eq!(snapshot(&connection, &id).unwrap(), after);
        assert!(
            apply_once(
                &mut connection,
                &id,
                &changes,
                &BTreeMap::new(),
                &request,
                &"0".repeat(64)
            )
            .is_err()
        );
        let failed_id = "e".repeat(32);
        assert!(
            apply_once(
                &mut connection,
                &id,
                &changes,
                &BTreeMap::new(),
                &failed_id,
                &sha
            )
            .is_err()
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM global_requests", [], |row| row
                    .get::<_, u32>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn checkpoint_migration_normalizes_sql_without_changing_rows_or_blob_bytes() {
        let source = Connection::open_in_memory().unwrap();
        source
            .execute_batch(
                &SCHEMA_DDL
                    .replace("CREATE TABLE IF NOT EXISTS", "CREATE TABLE")
                    .replace(
                        "session_json TEXT NOT NULL,created_at_utc TEXT NOT NULL",
                        "created_at_utc TEXT NOT NULL,session_json TEXT NOT NULL",
                    ),
            )
            .unwrap();
        source
            .execute("INSERT INTO schema_meta VALUES('schema_version','3')", [])
            .unwrap();
        let id = "a".repeat(32);
        source
            .execute(
                "INSERT INTO workspaces VALUES(?1,'C:\\fixture',1,NULL,60,'{}','now','now',NULL)",
                [&id],
            )
            .unwrap();
        source.execute("INSERT INTO session_intents(workspace_id,session_json,created_at_utc) VALUES(?1,'原始文字\r\n','now')",[&id]).unwrap();
        let blob = b"LF\nCRLF\r\n\0binary".repeat(70000);
        source.execute("INSERT INTO checkpoints VALUES(?1,'checkpoint','session','now','codex','{}',?2,'archive.zip',?2,'seal')",rusqlite::params![&id,&blob]).unwrap();
        assert!(validate_schema(&source).is_err());
        let mut destination = Connection::open_in_memory().unwrap();
        assert_eq!(
            normalize_owned(&source, &mut destination).unwrap().len(),
            64
        );
        validate_schema(&destination).unwrap();
        let migrated: Vec<u8> = destination
            .query_row("SELECT archive_blob FROM checkpoints", [], |row| row.get(0))
            .unwrap();
        assert_eq!(migrated, blob);
        assert_eq!(
            source
                .query_row("SELECT session_json FROM session_intents", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            destination
                .query_row("SELECT session_json FROM session_intents", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap()
        );
        assert!(normalize_owned(&source, &mut destination).is_err());
        source.execute_batch("CREATE TRIGGER unexpected AFTER DELETE ON sessions BEGIN DELETE FROM workspaces; END").unwrap();
        assert!(normalize_owned(&source, &mut Connection::open_in_memory().unwrap()).is_err());
    }
}
