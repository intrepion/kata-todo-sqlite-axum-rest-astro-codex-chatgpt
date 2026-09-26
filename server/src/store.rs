use crate::model::{
    ImportOutcome, TaskCounts, TaskExport, TaskSnapshot, TaskState, TaskView, list_etag,
    now_timestamp, task_counts, task_etag,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub struct Store {
    connection: Connection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryRecord {
    pub id: String,
    pub created_at: String,
    pub task_count: usize,
    pub snapshot_json: String,
}

pub enum ImportApply {
    Stale {
        list_etag: String,
        tasks: Vec<TaskSnapshot>,
    },
    Applied(ImportOutcome),
}

pub enum RecoveryApply {
    Stale {
        list_etag: String,
        tasks: Vec<TaskSnapshot>,
    },
    Applied {
        list_etag: String,
        recovery_point_id: String,
    },
}

impl Store {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS app_meta (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 revision INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO app_meta (id, revision) VALUES (1, 0);
             CREATE TABLE IF NOT EXISTS tasks (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 notes TEXT NOT NULL,
                 state TEXT NOT NULL CHECK (state IN ('active', 'completed', 'trashed')),
                 created_at TEXT NOT NULL,
                 completed_at TEXT,
                 trashed_at TEXT,
                 active_position INTEGER NOT NULL CHECK (active_position >= 0),
                 state_before_trash TEXT CHECK (state_before_trash IN ('active', 'completed')),
                 revision INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS tasks_active_order ON tasks (state, active_position);
             CREATE INDEX IF NOT EXISTS tasks_lifecycle_order ON tasks (state, completed_at, trashed_at);
             CREATE TABLE IF NOT EXISTS recovery_points (
                 id TEXT PRIMARY KEY,
                 created_at TEXT NOT NULL,
                 task_count INTEGER NOT NULL,
                 snapshot_json TEXT NOT NULL
             );",
        )?;
        Ok(Self { connection })
    }

    pub fn revision(&self) -> rusqlite::Result<i64> {
        self.connection
            .query_row("SELECT revision FROM app_meta WHERE id = 1", [], |row| {
                row.get(0)
            })
    }

    pub fn current_list_etag(&self) -> rusqlite::Result<String> {
        Ok(list_etag(self.revision()?))
    }

    pub fn list_tasks(&self) -> rusqlite::Result<Vec<TaskView>> {
        read_task_views(&self.connection)
    }

    pub fn counts(&self) -> rusqlite::Result<TaskCounts> {
        let tasks = read_snapshots(&self.connection)?;
        Ok(task_counts(&tasks))
    }

    pub fn export(&self) -> rusqlite::Result<TaskExport> {
        Ok(TaskExport {
            format_version: 1,
            exported_at: now_timestamp(),
            tasks: read_snapshots(&self.connection)?,
        })
    }

    pub fn create_task(
        &mut self,
        id: &str,
        title: &str,
        notes: &str,
    ) -> rusqlite::Result<TaskView> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = bump_revision(&tx)?;
        tx.execute(
            "UPDATE tasks SET active_position = active_position + 1, revision = ?1 WHERE state = 'active'",
            [revision],
        )?;
        tx.execute(
            "INSERT INTO tasks (id, title, notes, state, created_at, active_position, revision)
             VALUES (?1, ?2, ?3, 'active', ?4, 0, ?5)",
            params![id, title, notes, now_timestamp(), revision],
        )?;
        tx.commit()?;
        read_task_view(&self.connection, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn patch_task(
        &mut self,
        id: &str,
        if_match: &str,
        title: Option<&str>,
        notes: Option<&str>,
        target_state: Option<TaskState>,
    ) -> rusqlite::Result<Option<TaskView>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut current) = read_task_view(&tx, id)? else {
            return Ok(None);
        };
        if current.task.state == TaskState::Trashed {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let only_same_state =
            title.is_none() && notes.is_none() && target_state == Some(current.task.state);
        if only_same_state {
            tx.rollback()?;
            return Ok(Some(current));
        }
        if current.etag != if_match {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }

        let next_title = title.unwrap_or(&current.task.title).to_string();
        let next_notes = notes.unwrap_or(&current.task.notes).to_string();
        let next_state = target_state.unwrap_or(current.task.state);
        let mut active_position = current.task.active_position;
        let completed_at = match next_state {
            TaskState::Active => None,
            TaskState::Completed if current.task.state != TaskState::Completed => {
                Some(now_timestamp())
            }
            TaskState::Completed => current.task.completed_at.clone(),
            TaskState::Trashed => return Err(rusqlite::Error::InvalidQuery),
        };
        let revision = bump_revision(&tx)?;
        if current.task.state != TaskState::Active && next_state == TaskState::Active {
            active_position = insert_active_at(&tx, id, current.task.active_position, revision)?;
        }
        tx.execute(
            "UPDATE tasks SET title = ?1, notes = ?2, state = ?3, completed_at = ?4, active_position = ?5, revision = ?6 WHERE id = ?7",
            params![next_title, next_notes, next_state.as_str(), completed_at, active_position, revision, id],
        )?;
        if current.task.state == TaskState::Active && next_state != TaskState::Active {
            tx.execute(
                "UPDATE tasks SET active_position = active_position - 1, revision = ?1
                 WHERE state = 'active' AND active_position > ?2",
                params![revision, current.task.active_position],
            )?;
        }
        tx.commit()?;
        current =
            read_task_view(&self.connection, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        Ok(Some(current))
    }

    pub fn trash_task(&mut self, id: &str, if_match: &str) -> rusqlite::Result<Option<TaskView>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(task) = read_task_view(&tx, id)? else {
            return Ok(None);
        };
        if task.etag != if_match {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        if task.task.state == TaskState::Trashed {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let revision = bump_revision(&tx)?;
        tx.execute(
            "UPDATE tasks SET state_before_trash = state, state = 'trashed', trashed_at = ?1, revision = ?2 WHERE id = ?3",
            params![now_timestamp(), revision, id],
        )?;
        if task.task.state == TaskState::Active {
            tx.execute(
                "UPDATE tasks SET active_position = active_position - 1, revision = ?1
                 WHERE state = 'active' AND active_position > ?2",
                params![revision, task.task.active_position],
            )?;
        }
        tx.commit()?;
        read_task_view(&self.connection, id)
    }

    pub fn restore_task(&mut self, id: &str, if_match: &str) -> rusqlite::Result<Option<TaskView>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(task) = read_task_view(&tx, id)? else {
            return Ok(None);
        };
        if task.etag != if_match {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        if task.task.state != TaskState::Trashed {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let previous = task
            .task
            .state_before_trash
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let revision = bump_revision(&tx)?;
        let active_position = if previous == TaskState::Active {
            insert_active_at(&tx, id, task.task.active_position, revision)?
        } else {
            task.task.active_position
        };
        tx.execute(
            "UPDATE tasks SET state = ?1, state_before_trash = NULL, trashed_at = NULL, active_position = ?2, revision = ?3 WHERE id = ?4",
            params![previous.as_str(), active_position, revision, id],
        )?;
        tx.commit()?;
        read_task_view(&self.connection, id)
    }

    pub fn reorder(&mut self, ids: &[String], if_match: &str) -> Result<String, ReorderError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(ReorderError::Database)?;
        let revision = current_revision(&tx).map_err(ReorderError::Database)?;
        let current_etag = list_etag(revision);
        if current_etag != if_match {
            return Err(ReorderError::Stale(current_etag));
        }
        let current =
            read_snapshots_by_state(&tx, TaskState::Active).map_err(ReorderError::Database)?;
        let current_ids: std::collections::HashSet<&str> =
            current.iter().map(|task| task.id.as_str()).collect();
        let incoming_ids: std::collections::HashSet<&str> =
            ids.iter().map(String::as_str).collect();
        if ids.len() != current.len()
            || incoming_ids.len() != ids.len()
            || current_ids != incoming_ids
        {
            return Err(ReorderError::InvalidOrder);
        }
        let changed = ids.iter().enumerate().any(|(position, id)| {
            current
                .iter()
                .any(|task| task.id == *id && task.active_position != position as i64)
        });
        if !changed {
            tx.rollback().map_err(ReorderError::Database)?;
            return Ok(current_etag);
        }
        let next_revision = bump_revision(&tx).map_err(ReorderError::Database)?;
        for (position, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE tasks SET active_position = ?1, revision = ?2 WHERE id = ?3 AND state = 'active'",
                params![position as i64, next_revision, id],
            ).map_err(ReorderError::Database)?;
        }
        tx.commit().map_err(ReorderError::Database)?;
        Ok(list_etag(next_revision))
    }

    pub fn empty_trash(&mut self, if_match: &str) -> Result<(String, usize), StaleOrDatabase> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StaleOrDatabase::Database)?;
        let revision = current_revision(&tx).map_err(StaleOrDatabase::Database)?;
        let current_etag = list_etag(revision);
        if current_etag != if_match {
            return Err(StaleOrDatabase::Stale(current_etag));
        }
        let count = tx
            .execute("DELETE FROM tasks WHERE state = 'trashed'", [])
            .map_err(StaleOrDatabase::Database)?;
        let next_revision = if count > 0 {
            bump_revision(&tx).map_err(StaleOrDatabase::Database)?
        } else {
            revision
        };
        tx.commit().map_err(StaleOrDatabase::Database)?;
        Ok((list_etag(next_revision), count))
    }

    pub fn apply_import(
        &mut self,
        incoming: &TaskExport,
        if_match: &str,
        recovery_id: &str,
    ) -> rusqlite::Result<ImportApply> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = current_revision(&tx)?;
        if list_etag(revision) != if_match {
            let tasks = read_snapshots(&tx)?;
            return Ok(ImportApply::Stale {
                list_etag: list_etag(revision),
                tasks,
            });
        }
        let prior = snapshot_for(&tx)?;
        insert_recovery_point(&tx, recovery_id, &prior)?;
        let next_revision = replace_tasks(&tx, &incoming.tasks)?;
        tx.commit()?;
        Ok(ImportApply::Applied(ImportOutcome {
            applied: true,
            recovery_point_id: recovery_id.to_string(),
            list_etag: list_etag(next_revision),
        }))
    }

    pub fn recovery_points(&self) -> rusqlite::Result<Vec<RecoveryRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT id, created_at, task_count, snapshot_json FROM recovery_points ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(RecoveryRecord {
                id: row.get(0)?,
                created_at: row.get(1)?,
                task_count: row.get::<_, i64>(2)? as usize,
                snapshot_json: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn recovery_point(&self, id: &str) -> rusqlite::Result<Option<RecoveryRecord>> {
        self.connection.query_row(
            "SELECT id, created_at, task_count, snapshot_json FROM recovery_points WHERE id = ?1",
            [id],
            |row| Ok(RecoveryRecord {
                id: row.get(0)?, created_at: row.get(1)?, task_count: row.get::<_, i64>(2)? as usize,
                snapshot_json: row.get(3)?,
            }),
        ).optional()
    }

    pub fn restore_recovery_point(
        &mut self,
        id: &str,
        if_match: &str,
        recovery_id: &str,
    ) -> rusqlite::Result<RecoveryApply> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = current_revision(&tx)?;
        if list_etag(revision) != if_match {
            let tasks = read_snapshots(&tx)?;
            return Ok(RecoveryApply::Stale {
                list_etag: list_etag(revision),
                tasks,
            });
        }
        let snapshot_json: Option<String> = tx
            .query_row(
                "SELECT snapshot_json FROM recovery_points WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(snapshot_json) = snapshot_json else {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        };
        let target: TaskExport = serde_json::from_str(&snapshot_json)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let prior = snapshot_for(&tx)?;
        insert_recovery_point(&tx, recovery_id, &prior)?;
        let next_revision = replace_tasks(&tx, &target.tasks)?;
        tx.commit()?;
        Ok(RecoveryApply::Applied {
            list_etag: list_etag(next_revision),
            recovery_point_id: recovery_id.to_string(),
        })
    }

    pub fn delete_recovery_point(&mut self, id: &str) -> rusqlite::Result<bool> {
        Ok(self
            .connection
            .execute("DELETE FROM recovery_points WHERE id = ?1", [id])?
            > 0)
    }
}

pub enum ReorderError {
    Stale(String),
    InvalidOrder,
    Database(rusqlite::Error),
}
pub enum StaleOrDatabase {
    Stale(String),
    Database(rusqlite::Error),
}

fn insert_active_at(
    tx: &Transaction<'_>,
    id: &str,
    saved_position: i64,
    revision: i64,
) -> rusqlite::Result<i64> {
    let active_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM tasks WHERE state = 'active'",
        [],
        |row| row.get(0),
    )?;
    let position = saved_position.min(active_count);
    tx.execute(
        "UPDATE tasks SET active_position = active_position + 1, revision = ?1
         WHERE state = 'active' AND active_position >= ?2 AND id != ?3",
        params![revision, position, id],
    )?;
    Ok(position)
}

fn current_revision(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row("SELECT revision FROM app_meta WHERE id = 1", [], |row| {
        row.get(0)
    })
}

fn bump_revision(tx: &Transaction<'_>) -> rusqlite::Result<i64> {
    tx.execute(
        "UPDATE app_meta SET revision = revision + 1 WHERE id = 1",
        [],
    )?;
    current_revision(tx)
}

fn read_task_views(connection: &Connection) -> rusqlite::Result<Vec<TaskView>> {
    let mut statement = connection.prepare(
        "SELECT id, title, notes, state, created_at, completed_at, trashed_at, active_position, state_before_trash, revision
         FROM tasks ORDER BY
           CASE state WHEN 'active' THEN 0 WHEN 'completed' THEN 1 ELSE 2 END,
           CASE WHEN state = 'active' THEN active_position END ASC,
           CASE WHEN state = 'completed' THEN completed_at END DESC,
           CASE WHEN state = 'trashed' THEN trashed_at END DESC,
           id ASC",
    )?;
    let rows = statement.query_map([], row_to_view)?;
    rows.collect()
}

fn read_task_view(connection: &Connection, id: &str) -> rusqlite::Result<Option<TaskView>> {
    connection.query_row(
        "SELECT id, title, notes, state, created_at, completed_at, trashed_at, active_position, state_before_trash, revision FROM tasks WHERE id = ?1",
        [id], row_to_view,
    ).optional()
}

fn read_snapshots(connection: &Connection) -> rusqlite::Result<Vec<TaskSnapshot>> {
    Ok(read_task_views(connection)?
        .into_iter()
        .map(|view| view.task)
        .collect())
}

fn read_snapshots_by_state(
    connection: &Connection,
    state: TaskState,
) -> rusqlite::Result<Vec<TaskSnapshot>> {
    Ok(read_task_views(connection)?
        .into_iter()
        .map(|view| view.task)
        .filter(|task| task.state == state)
        .collect())
}

fn row_to_view(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskView> {
    let id: String = row.get(0)?;
    let revision: i64 = row.get(9)?;
    Ok(TaskView {
        etag: task_etag(&id, revision),
        task: TaskSnapshot {
            id,
            title: row.get(1)?,
            notes: row.get(2)?,
            state: parse_state(&row.get::<_, String>(3)?)?,
            created_at: row.get(4)?,
            completed_at: row.get(5)?,
            trashed_at: row.get(6)?,
            active_position: row.get(7)?,
            state_before_trash: row
                .get::<_, Option<String>>(8)?
                .map(|state| parse_state(&state))
                .transpose()?,
        },
    })
}

fn parse_state(value: &str) -> rusqlite::Result<TaskState> {
    match value {
        "active" => Ok(TaskState::Active),
        "completed" => Ok(TaskState::Completed),
        "trashed" => Ok(TaskState::Trashed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn snapshot_for(connection: &Connection) -> rusqlite::Result<TaskExport> {
    Ok(TaskExport {
        format_version: 1,
        exported_at: now_timestamp(),
        tasks: read_snapshots(connection)?,
    })
}

fn insert_recovery_point(
    tx: &Transaction<'_>,
    id: &str,
    snapshot: &TaskExport,
) -> rusqlite::Result<()> {
    let serialized = serde_json::to_string(snapshot)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    tx.execute(
        "INSERT INTO recovery_points (id, created_at, task_count, snapshot_json) VALUES (?1, ?2, ?3, ?4)",
        params![id, now_timestamp(), snapshot.tasks.len() as i64, serialized],
    )?;
    Ok(())
}

fn replace_tasks(tx: &Transaction<'_>, tasks: &[TaskSnapshot]) -> rusqlite::Result<i64> {
    tx.execute("DELETE FROM tasks", [])?;
    let revision = bump_revision(tx)?;
    for task in tasks {
        tx.execute(
            "INSERT INTO tasks (id, title, notes, state, created_at, completed_at, trashed_at, active_position, state_before_trash, revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task.id, task.title.trim(), task.notes, task.state.as_str(), task.created_at,
                task.completed_at, task.trashed_at, task.active_position,
                task.state_before_trash.map(TaskState::as_str), revision,
            ],
        )?;
    }
    Ok(revision)
}
