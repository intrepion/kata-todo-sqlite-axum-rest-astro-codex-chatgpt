use crate::{
    model::*,
    store::{ImportApply, RecoveryApply, RecoveryRecord, ReorderError, StaleOrDatabase, Store},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    staged_imports: Arc<Mutex<HashMap<String, PendingImport>>>,
}

struct PendingImport {
    snapshot: Option<TaskExport>,
    preview: ImportPreview,
    outcome: Option<ImportOutcome>,
}

#[derive(Deserialize)]
struct TaskQuery {
    state: Option<String>,
    q: Option<String>,
}

#[derive(Deserialize)]
struct OrderRequest {
    ids: Vec<String>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    fields: Option<Value>,
    extra: Option<Value>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            fields: None,
            extra: None,
        }
    }

    fn field(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        field: &str,
        detail: &str,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            fields: Some(json!({field: detail})),
            extra: None,
        }
    }

    fn with_extra(mut self, extra: Value) -> Self {
        self.extra = Some(extra);
        self
    }

    fn database() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "The server could not complete the request.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(fields) = self.fields {
            error["fields"] = fields;
        }
        let mut body = json!({"error": error});
        if let Some(extra) = self.extra {
            if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
                target.extend(source.clone());
            }
        }
        (self.status, Json(body)).into_response()
    }
}

pub fn router(store: Store, web_dir: PathBuf) -> Router {
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        staged_imports: Arc::new(Mutex::new(HashMap::new())),
    };
    let api = Router::new()
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{id}", patch(update_task).delete(trash_task))
        .route("/api/tasks/{id}/restore", post(restore_task))
        .route("/api/tasks/order", put(reorder_tasks))
        .route("/api/trash", delete(empty_trash))
        .route("/api/export", get(export_tasks))
        .route("/api/import-previews", post(create_import_preview))
        .route(
            "/api/import-previews/{id}/apply",
            post(apply_import_preview),
        )
        .route("/api/import-previews/{id}", delete(discard_import_preview))
        .route("/api/recovery-points", get(list_recovery_points))
        .route(
            "/api/recovery-points/{id}",
            get(get_recovery_point).delete(delete_recovery_point),
        )
        .route(
            "/api/recovery-points/{id}/restore",
            post(restore_recovery_point),
        )
        .with_state(state)
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(middleware::from_fn(same_origin_guard));
    api.fallback_service(ServeDir::new(web_dir).append_index_html_on_directories(true))
}

async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<TaskQuery>,
) -> Result<Response, ApiError> {
    let store = lock_store(&state)?;
    let etag = store
        .current_list_etag()
        .map_err(|_| ApiError::database())?;
    let counts = store.counts().map_err(|_| ApiError::database())?;
    let mut tasks = store.list_tasks().map_err(|_| ApiError::database())?;
    if let Some(query_text) = query.q.filter(|query| !query.is_empty()) {
        let needle = query_text.to_lowercase();
        tasks.retain(|task| {
            task.task.title.to_lowercase().contains(&needle)
                || task.task.notes.to_lowercase().contains(&needle)
        });
    } else if let Some(filter) = query.state {
        let expected = match filter.as_str() {
            "active" => TaskState::Active,
            "completed" => TaskState::Completed,
            "trashed" => TaskState::Trashed,
            _ => {
                return Err(ApiError::field(
                    StatusCode::BAD_REQUEST,
                    "invalid_state",
                    "State must be active, completed, or trashed.",
                    "state",
                    "invalid value",
                ));
            }
        };
        tasks.retain(|task| task.task.state == expected);
    }
    let mut response = Json(TaskList { tasks, counts }).into_response();
    set_etag(response.headers_mut(), &etag);
    Ok(response)
}

async fn create_task(State(state): State<AppState>, body: Bytes) -> Result<Response, ApiError> {
    let request: CreateTask = parse_json(&body)?;
    let title = validate_title(&request.title)?;
    validate_notes(&request.notes)?;
    let mut store = lock_store_mut(&state)?;
    let task = store
        .create_task(&new_id(), &title, &request.notes)
        .map_err(|_| ApiError::database())?;
    Ok(task_response(task))
}

async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request: PatchTask = parse_json(&body)?;
    if request.title.is_none() && request.notes.is_none() && request.state.is_none() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "empty_update",
            "Provide a title, notes, or state to update.",
        ));
    }
    let title = request.title.as_deref().map(validate_title).transpose()?;
    if let Some(notes) = request.notes.as_deref() {
        validate_notes(notes)?;
    }
    if request.state == Some(TaskState::Trashed) {
        return Err(ApiError::field(
            StatusCode::BAD_REQUEST,
            "invalid_state",
            "Tasks can only be active or completed through this operation.",
            "state",
            "invalid value",
        ));
    }
    let if_match = required_if_match(&headers)?;
    let mut store = lock_store_mut(&state)?;
    match store.patch_task(
        &id,
        &if_match,
        title.as_deref(),
        request.notes.as_deref(),
        request.state,
    ) {
        Ok(Some(task)) => Ok(task_response(task)),
        Ok(None) => Err(not_found("task_not_found", "Task was not found.")),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(stale_error()),
        Err(rusqlite::Error::InvalidQuery) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "task_in_trash",
            "Restore this task before editing it.",
        )),
        Err(_) => Err(ApiError::database()),
    }
}

async fn trash_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let if_match = required_if_match(&headers)?;
    let mut store = lock_store_mut(&state)?;
    match store.trash_task(&id, &if_match) {
        Ok(Some(task)) => Ok(task_response(task)),
        Ok(None) => Err(not_found("task_not_found", "Task was not found.")),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(stale_error()),
        Err(rusqlite::Error::InvalidQuery) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "task_in_trash",
            "Task is already in Trash.",
        )),
        Err(_) => Err(ApiError::database()),
    }
}

async fn restore_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let if_match = required_if_match(&headers)?;
    let mut store = lock_store_mut(&state)?;
    match store.restore_task(&id, &if_match) {
        Ok(Some(task)) => Ok(task_response(task)),
        Ok(None) => Err(not_found("task_not_found", "Task was not found.")),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(stale_error()),
        Err(rusqlite::Error::InvalidQuery) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "task_not_trashed",
            "Only tasks in Trash can be restored.",
        )),
        Err(_) => Err(ApiError::database()),
    }
}

async fn reorder_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request: OrderRequest = parse_json(&body)?;
    let if_match = required_if_match(&headers)?;
    let mut store = lock_store_mut(&state)?;
    match store.reorder(&request.ids, &if_match) {
        Ok(etag) => Ok((
            [(header::ETAG, header_value(&etag)?)],
            Json(json!({"etag": etag})),
        )
            .into_response()),
        Err(ReorderError::Stale(etag)) => Err(stale_error_with_etag(etag)),
        Err(ReorderError::InvalidOrder) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_order",
            "IDs must contain every active task exactly once.",
        )),
        Err(ReorderError::Database(error)) => {
            tracing::error!(?error, "reorder failed");
            Err(ApiError::database())
        }
    }
}

async fn empty_trash(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let if_match = required_if_match(&headers)?;
    let mut store = lock_store_mut(&state)?;
    match store.empty_trash(&if_match) {
        Ok((etag, deleted)) => Ok((
            [(header::ETAG, header_value(&etag)?)],
            Json(json!({"deleted": deleted, "etag": etag})),
        )
            .into_response()),
        Err(StaleOrDatabase::Stale(etag)) => Err(stale_error_with_etag(etag)),
        Err(StaleOrDatabase::Database(error)) => {
            tracing::error!(?error, "empty Trash failed");
            Err(ApiError::database())
        }
    }
}

async fn export_tasks(State(state): State<AppState>) -> Result<Response, ApiError> {
    let store = lock_store(&state)?;
    let export = store.export().map_err(|_| ApiError::database())?;
    let mut response = Json(export).into_response();
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=todo-list.json"),
    );
    Ok(response)
}

async fn create_import_preview(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let mut export: TaskExport = parse_json(&body)?;
    validate_export(&export).map_err(|(code, message)| {
        ApiError::new(StatusCode::BAD_REQUEST, code_leak(&code), message)
    })?;
    for task in &mut export.tasks {
        task.title = task.title.trim().to_string();
        task.created_at = normalize_timestamp(&task.created_at).expect("validated timestamp");
        task.completed_at = task.completed_at.as_deref().and_then(normalize_timestamp);
        task.trashed_at = task.trashed_at.as_deref().and_then(normalize_timestamp);
    }
    export.exported_at = normalize_timestamp(&export.exported_at).expect("validated timestamp");
    let id = new_id();
    let store = lock_store(&state)?;
    let current_tasks: Vec<TaskSnapshot> = store
        .list_tasks()
        .map_err(|_| ApiError::database())?
        .into_iter()
        .map(|view| view.task)
        .collect();
    let preview = ImportPreview {
        id: id.clone(),
        current_counts: task_counts(&current_tasks),
        incoming_counts: task_counts(&export.tasks),
        incoming_tasks: export.tasks.clone(),
        list_etag: store
            .current_list_etag()
            .map_err(|_| ApiError::database())?,
    };
    drop(store);
    state
        .staged_imports
        .lock()
        .map_err(|_| ApiError::database())?
        .insert(
            id,
            PendingImport {
                snapshot: Some(export),
                preview: preview.clone(),
                outcome: None,
            },
        );
    Ok(Json(preview).into_response())
}

async fn apply_import_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let supplied_etag = required_if_match(&headers)?;
    let mut staged = state
        .staged_imports
        .lock()
        .map_err(|_| ApiError::database())?;
    let Some(pending) = staged.get_mut(&id) else {
        return Err(not_found(
            "preview_not_found",
            "Import preview was not found.",
        ));
    };
    if let Some(outcome) = &pending.outcome {
        let mut response = Json(outcome).into_response();
        set_etag(response.headers_mut(), &outcome.list_etag);
        return Ok(response);
    }
    let Some(snapshot) = pending.snapshot.as_ref() else {
        return Err(not_found(
            "preview_not_found",
            "Import preview was not found.",
        ));
    };
    let expected_etag = pending.preview.list_etag.clone();
    let snapshot = snapshot.clone();
    let result = {
        let mut store = lock_store_mut(&state)?;
        if supplied_etag != expected_etag {
            let list_etag = store
                .current_list_etag()
                .map_err(|_| ApiError::database())?;
            let tasks = store
                .list_tasks()
                .map_err(|_| ApiError::database())?
                .into_iter()
                .map(|view| view.task)
                .collect();
            ImportApply::Stale { list_etag, tasks }
        } else {
            store
                .apply_import(&snapshot, &expected_etag, &new_id())
                .map_err(|_| ApiError::database())?
        }
    };
    match result {
        ImportApply::Applied(outcome) => {
            pending.snapshot = None;
            pending.preview.incoming_tasks.clear();
            pending.outcome = Some(outcome.clone());
            let mut response = Json(outcome.clone()).into_response();
            set_etag(response.headers_mut(), &outcome.list_etag);
            Ok(response)
        }
        ImportApply::Stale { list_etag, tasks } => {
            pending.preview.list_etag = list_etag;
            pending.preview.current_counts = task_counts(&tasks);
            let preview = pending.preview.clone();
            Err(ApiError::new(StatusCode::PRECONDITION_FAILED, "stale_list", "The To Do List changed after this preview. Review the updated preview and confirm again.")
                .with_extra(json!({"preview": preview})))
        }
    }
}

async fn discard_import_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .staged_imports
        .lock()
        .map_err(|_| ApiError::database())?
        .remove(&id);
    Ok(StatusCode::NO_CONTENT)
}

async fn list_recovery_points(
    State(state): State<AppState>,
) -> Result<Json<Vec<RecoveryPointSummary>>, ApiError> {
    let store = lock_store(&state)?;
    let records = store.recovery_points().map_err(|_| ApiError::database())?;
    Ok(Json(records.iter().map(summary).collect()))
}

async fn get_recovery_point(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let store = lock_store(&state)?;
    let record = store
        .recovery_point(&id)
        .map_err(|_| ApiError::database())?
        .ok_or_else(|| not_found("recovery_point_not_found", "Recovery point was not found."))?;
    let export = parse_recovery_snapshot(&record)?;
    let preview = RecoveryPointPreview {
        recovery_point: summary(&record),
        counts: task_counts(&export.tasks),
        tasks: export.tasks,
        list_etag: store
            .current_list_etag()
            .map_err(|_| ApiError::database())?,
    };
    let mut response = Json(preview.clone()).into_response();
    set_etag(response.headers_mut(), &preview.list_etag);
    Ok(response)
}

async fn restore_recovery_point(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let if_match = required_if_match(&headers)?;
    let recovery_id = new_id();
    let mut store = lock_store_mut(&state)?;
    let selected = store
        .recovery_point(&id)
        .map_err(|_| ApiError::database())?
        .ok_or_else(|| not_found("recovery_point_not_found", "Recovery point was not found."))?;
    let selected_export = parse_recovery_snapshot(&selected)?;
    match store
        .restore_recovery_point(&id, &if_match, &recovery_id)
        .map_err(|_| ApiError::database())?
    {
        RecoveryApply::Applied {
            list_etag,
            recovery_point_id,
        } => {
            let result = json!({"restored": true, "recovery_point_id": recovery_point_id, "list_etag": list_etag});
            let mut response = Json(result).into_response();
            set_etag(response.headers_mut(), &list_etag);
            Ok(response)
        }
        RecoveryApply::Stale { list_etag, tasks } => {
            let preview = json!({
                "recovery_point": summary(&selected),
                "current_counts": task_counts(&tasks),
                "current_tasks": tasks,
                "incoming_counts": task_counts(&selected_export.tasks),
                "incoming_tasks": selected_export.tasks,
                "list_etag": list_etag,
            });
            Err(ApiError::new(StatusCode::PRECONDITION_FAILED, "stale_list", "The To Do List changed after this preview. Review the updated preview and confirm again.")
                .with_extra(json!({"preview": preview})))
        }
    }
}

async fn delete_recovery_point(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut store = lock_store_mut(&state)?;
    if store
        .delete_recovery_point(&id)
        .map_err(|_| ApiError::database())?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found(
            "recovery_point_not_found",
            "Recovery point was not found.",
        ))
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            "Request body must be valid JSON.",
        )
    })
}

fn validate_title(value: &str) -> Result<String, ApiError> {
    let title = value.trim();
    if title.is_empty() {
        return Err(ApiError::field(
            StatusCode::BAD_REQUEST,
            "invalid_title",
            "Title must contain non-whitespace text.",
            "title",
            "required",
        ));
    }
    if title.chars().count() > 500 {
        return Err(ApiError::field(
            StatusCode::BAD_REQUEST,
            "invalid_title",
            "Title may contain at most 500 characters.",
            "title",
            "too long",
        ));
    }
    Ok(title.to_string())
}

fn validate_notes(notes: &str) -> Result<(), ApiError> {
    if notes.chars().count() > 20_000 {
        return Err(ApiError::field(
            StatusCode::BAD_REQUEST,
            "invalid_notes",
            "Notes may contain at most 20,000 characters.",
            "notes",
            "too long",
        ));
    }
    Ok(())
}

fn required_if_match(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::PRECONDITION_REQUIRED,
                "if_match_required",
                "Send the current ETag in If-Match before changing this resource.",
            )
        })
}

fn task_response(task: TaskView) -> Response {
    let etag = task.etag.clone();
    let mut response = Json(task).into_response();
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

fn set_etag(headers: &mut HeaderMap, etag: &str) {
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
}

fn header_value(value: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(value).map_err(|_| ApiError::database())
}

fn lock_store(state: &AppState) -> Result<std::sync::MutexGuard<'_, Store>, ApiError> {
    state.store.lock().map_err(|_| ApiError::database())
}

fn lock_store_mut(state: &AppState) -> Result<std::sync::MutexGuard<'_, Store>, ApiError> {
    lock_store(state)
}

fn stale_error() -> ApiError {
    ApiError::new(
        StatusCode::PRECONDITION_FAILED,
        "stale_resource",
        "This resource changed. Refresh it and review the latest version before retrying.",
    )
}

fn stale_error_with_etag(etag: String) -> ApiError {
    ApiError::new(
        StatusCode::PRECONDITION_FAILED,
        "stale_resource",
        "This resource changed. Refresh it and review the latest version before retrying.",
    )
    .with_extra(json!({"current_etag": etag}))
}

fn not_found(code: &'static str, message: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, code, message)
}

fn summary(record: &RecoveryRecord) -> RecoveryPointSummary {
    RecoveryPointSummary {
        id: record.id.clone(),
        created_at: record.created_at.clone(),
        task_count: record.task_count,
    }
}

fn parse_recovery_snapshot(record: &RecoveryRecord) -> Result<TaskExport, ApiError> {
    serde_json::from_str(&record.snapshot_json).map_err(|_| ApiError::database())
}

fn code_leak(code: &str) -> &'static str {
    match code {
        "unsupported_format" => "unsupported_format",
        "invalid_title" => "invalid_title",
        "invalid_notes" => "invalid_notes",
        "invalid_task_id" => "invalid_task_id",
        "invalid_order" => "invalid_order",
        _ => "invalid_export",
    }
}

async fn same_origin_guard(request: Request, next: Next) -> Response {
    let headers = request.headers();
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        let request_host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok());
        let same_host = origin.strip_prefix("http://").is_some_and(|authority| {
            let hostname = authority
                .rsplit_once(':')
                .map_or(authority, |(host, _)| host);
            Some(authority) == request_host && matches!(hostname, "127.0.0.1" | "localhost")
        });
        let local_dev = matches!(origin, "http://127.0.0.1:4321" | "http://localhost:4321");
        if !same_host && !local_dev {
            return ApiError::new(
                StatusCode::FORBIDDEN,
                "cross_origin_request",
                "Requests must come from this local application.",
            )
            .into_response();
        }
    } else if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        == Some("cross-site")
    {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "cross_origin_request",
            "Requests must come from this local application.",
        )
        .into_response();
    }
    next.run(request).await
}
