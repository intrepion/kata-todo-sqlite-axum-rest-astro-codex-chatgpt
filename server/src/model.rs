use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Active,
    Completed,
    Trashed,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Trashed => "trashed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSnapshot {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub state: TaskState,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub trashed_at: Option<String>,
    pub active_position: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_before_trash: Option<TaskState>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskExport {
    pub format_version: u32,
    pub exported_at: String,
    pub tasks: Vec<TaskSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: TaskSnapshot,
    pub etag: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct TaskCounts {
    pub active: usize,
    pub completed: usize,
    pub trashed: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskList {
    pub tasks: Vec<TaskView>,
    pub counts: TaskCounts,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateTask {
    pub title: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PatchTask {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub state: Option<TaskState>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecoveryPointSummary {
    pub id: String,
    pub created_at: String,
    pub task_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecoveryPointPreview {
    pub recovery_point: RecoveryPointSummary,
    pub counts: TaskCounts,
    pub tasks: Vec<TaskSnapshot>,
    pub list_etag: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportPreview {
    pub id: String,
    pub current_counts: TaskCounts,
    pub incoming_counts: TaskCounts,
    pub incoming_tasks: Vec<TaskSnapshot>,
    pub list_etag: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportOutcome {
    pub applied: bool,
    pub recovery_point_id: String,
    pub list_etag: String,
}

pub fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

pub fn normalize_timestamp(value: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .filter(|timestamp| timestamp.offset().local_minus_utc() == 0)
        .map(|timestamp| {
            timestamp
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Nanos, true)
        })
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn list_etag(revision: i64) -> String {
    format!("\"list:{revision}\"")
}

pub fn task_etag(id: &str, revision: i64) -> String {
    format!("\"task:{id}:{revision}\"")
}

pub fn task_counts(tasks: &[TaskSnapshot]) -> TaskCounts {
    let mut counts = TaskCounts::default();
    for task in tasks {
        match task.state {
            TaskState::Active => counts.active += 1,
            TaskState::Completed => counts.completed += 1,
            TaskState::Trashed => counts.trashed += 1,
        }
    }
    counts
}

pub fn validate_export(export: &TaskExport) -> Result<(), (String, String)> {
    if export.format_version != 1 {
        return Err((
            "unsupported_format".into(),
            format!("Unsupported task export version {}.", export.format_version),
        ));
    }
    if !valid_timestamp(&export.exported_at) {
        return Err((
            "invalid_export".into(),
            "exported_at must be an RFC 3339 UTC timestamp.".into(),
        ));
    }

    let mut ids = std::collections::HashSet::new();
    let mut active_positions = std::collections::HashSet::new();
    let mut active_count = 0usize;
    for task in &export.tasks {
        let title = task.title.trim();
        if title.is_empty() || title.chars().count() > 500 {
            return Err((
                "invalid_title".into(),
                "Titles must contain 1 to 500 non-whitespace characters.".into(),
            ));
        }
        if task.notes.chars().count() > 20_000 {
            return Err((
                "invalid_notes".into(),
                "Notes may contain at most 20,000 characters.".into(),
            ));
        }
        if Uuid::parse_str(&task.id).is_err() || !ids.insert(&task.id) {
            return Err((
                "invalid_task_id".into(),
                "Task IDs must be unique UUIDs.".into(),
            ));
        }
        if task.active_position < 0 || !valid_timestamp(&task.created_at) {
            return Err((
                "invalid_task".into(),
                "Task position and timestamps are invalid.".into(),
            ));
        }
        for timestamp in [&task.completed_at, &task.trashed_at].into_iter().flatten() {
            if !valid_timestamp(timestamp) {
                return Err((
                    "invalid_task".into(),
                    "Task timestamps must use RFC 3339 UTC.".into(),
                ));
            }
        }
        match task.state {
            TaskState::Active => {
                if task.completed_at.is_some()
                    || task.trashed_at.is_some()
                    || task.state_before_trash.is_some()
                {
                    return Err((
                        "invalid_task".into(),
                        "Active tasks cannot have completion or trash state.".into(),
                    ));
                }
                active_count += 1;
                if !active_positions.insert(task.active_position) {
                    return Err((
                        "invalid_order".into(),
                        "Active task positions must be unique.".into(),
                    ));
                }
            }
            TaskState::Completed => {
                if task.completed_at.is_none()
                    || task.trashed_at.is_some()
                    || task.state_before_trash.is_some()
                {
                    return Err((
                        "invalid_task".into(),
                        "Completed tasks require completed_at and cannot have trash state.".into(),
                    ));
                }
            }
            TaskState::Trashed => {
                if task.trashed_at.is_none()
                    || !matches!(
                        task.state_before_trash,
                        Some(TaskState::Active | TaskState::Completed)
                    )
                    || (task.state_before_trash == Some(TaskState::Completed))
                        != task.completed_at.is_some()
                {
                    return Err((
                        "invalid_task".into(),
                        "Trashed tasks require a previous state and trashed_at.".into(),
                    ));
                }
            }
        }
    }
    if active_positions.len() != active_count
        || (0..active_count as i64).any(|position| !active_positions.contains(&position))
    {
        return Err((
            "invalid_order".into(),
            "Active positions must run from zero without gaps.".into(),
        ));
    }
    Ok(())
}

fn valid_timestamp(value: &str) -> bool {
    normalize_timestamp(value).is_some()
}
