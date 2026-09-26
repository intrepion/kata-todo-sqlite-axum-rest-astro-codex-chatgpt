# REST API Contract

This document records the REST operations and wire behavior implemented by the To Do List application.

## Tasks

| Method and path | Behavior |
| --- | --- |
| `GET /api/tasks` | Return `{ "tasks": [...], "counts": {"active": 0, "completed": 0, "trashed": 0} }`. A `state` filter selects Active, Completed, or Trash. A `q` query searches titles and notes across all states and labels each result with its state. The collection ETag is in the response header. |
| `POST /api/tasks` | Create a task in Active from `{ "title": "...", "notes": "..." }`; `notes` may be omitted. Return the created task and its ETag. |
| `PATCH /api/tasks/{id}` | Edit `title` or `notes`, or explicitly set `state` to `active` or `completed`. Setting the same state repeatedly has the same result. Send the task's ETag in `If-Match`; a stale write is rejected. |
| `DELETE /api/tasks/{id}` | Move the task to Trash, preserving its previous state and position. Send the task's ETag in `If-Match`; return the task in its new state. |
| `POST /api/tasks/{id}/restore` | Restore a trashed task to its previous state and position. Send its ETag in `If-Match`; return the restored task. |
| `PUT /api/tasks/order` | Set the ordered IDs of active tasks atomically using `{ "ids": ["..."] }` and the collection ETag in `If-Match`. |
| `DELETE /api/trash` | Permanently delete all trashed tasks using the collection ETag in `If-Match`. |

## Export, import, and recovery history

| Method and path | Behavior |
| --- | --- |
| `GET /api/export` | Return a JSON export of task data, excluding local recovery history. |
| `POST /api/import-previews` | Validate and stage a version 1 JSON snapshot. Return an immutable preview ID, current and incoming counts by state, the incoming tasks, and the current To Do List ETag. Unsupported versions and invalid input leave current data unchanged. |
| `POST /api/import-previews/{id}/apply` | After user confirmation, replace current task data with the staged snapshot and retain the prior state as a recovery point. Send the preview's To Do List ETag in `If-Match`. If the list changed after preview, return `412` with a refreshed preview and require confirmation again. A preview can be applied once; retries return the original outcome. |
| `DELETE /api/import-previews/{id}` | Discard a staged import preview without changing task data. |
| `GET /api/recovery-points` | List recovery points with their creation date and task count. |
| `GET /api/recovery-points/{id}` | Return the recovery point's counts and task list for preview, plus the current To Do List ETag. |
| `POST /api/recovery-points/{id}/restore` | Restore the selected recovery point, preserving the state being replaced as a new recovery point. Send the current To Do List ETag in `If-Match`; if the list changed after preview, return `412` with the new current ETag and preview details so the user can confirm again. |
| `DELETE /api/recovery-points/{id}` | Delete a recovery point. |

## Validation and conflicts

- Task titles are trimmed and must contain at least one non-whitespace character. Notes may be empty and preserve line breaks.
- Titles are limited to 500 characters, notes to 20,000 characters, and import files to 10 MiB.
- Requests that change task completion set the target state explicitly and are safe to repeat.
- Task writes include a revision. A write based on a stale revision is rejected without changing the task, so another tab's edit is not silently lost. Setting a task to its existing completion state is a no-op and remains retry-safe.
- Task responses carry an `ETag`; task writes send the current tag in `If-Match`. Stale writes receive `412 Precondition Failed`.
- Reorder and Trash-wide writes use the corresponding collection ETag in `If-Match`.
- Import apply and recovery restore use the current To Do List ETag. If it is stale, the server changes nothing and the client must present a refreshed preview for confirmation.
- API errors use `{"error":{"code":"...","message":"..."}}`, with field details when relevant. Invalid requests do not change data. Invalid input uses `400`, missing resources use `404`, conflicting task state uses `409`, a missing `If-Match` uses `428`, and stale revisions use `412`.
- Cross-origin browser requests are rejected with `403`; the normal app origin and the local Astro development origin are allowed.
- Staged import previews exist until applied, discarded, or server restart. Recovery points are listed newest first. Successful state-changing responses return the resource ETag in the HTTP header; task collection responses also include each task's ETag in its JSON object.

## JSON task export shape

Each regular export is a versioned JSON object with `format_version: 1`, an `exported_at` UTC timestamp, and a `tasks` array. Version 1 task objects contain a UUID `id`, `title`, `notes`, `state` (`active`, `completed`, or `trashed`), `created_at`, nullable `completed_at` and `trashed_at`, and `active_position`. A trashed task also has `state_before_trash` (`active` or `completed`). Completed and trashed tasks retain the active-list position needed to return to Active. Timestamps use RFC 3339 UTC. HTTP ETags are local concurrency tokens and are not exported. The current server accepts version 1 and rejects all unsupported versions.

## Error response

Errors use one JSON shape. `fields` is included when a particular input field needs correction:

```json
{
  "error": {
    "code": "invalid_title",
    "message": "Title must contain non-whitespace text.",
    "fields": { "title": "required" }
  }
}
```

## Import preview behavior

An import preview remains available in server memory until it is applied, discarded, or the server restarts. Applying a preview stores the pre-import state as a recovery point. The staged snapshot is consumed after apply, while a small outcome record remains until restart so retries of the same preview ID return the original result without another replacement or recovery point.

If the current To Do List ETag changed after preview, apply returns `412 Precondition Failed` with a refreshed preview bound to the same incoming snapshot and the new list ETag. The client shows the refreshed counts and task list and asks the user to confirm again. Recovery-point restore follows the same stale-list rule and returns both the current list and the unchanged target recovery snapshot so the user can review the replacement again.

## ETags and errors

List responses use an ETag of the form `"list:<revision>"`; task objects and individual task responses use `"task:<uuid>:<revision>"`. A changed list advances its collection revision. Reorder and empty-Trash requests use the list ETag; edits, completion changes, Trash, and individual restore use the task ETag.

Error bodies have a stable `error.code` and human-readable `error.message`. When applicable they include `error.fields`. A stale task write returns `412` with `stale_resource`. A stale import or recovery restore returns `412` with `stale_list` and a `preview` object containing the refreshed list ETag and task counts. Invalid requests are rejected before mutation.
