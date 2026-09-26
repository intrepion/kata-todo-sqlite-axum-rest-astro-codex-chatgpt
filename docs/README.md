# Daymark To Do List

A single-person local To Do List learning kata. Daymark keeps task data in SQLite on this computer and provides a browser interface through a local Rust server.

## Run the application

Requirements: Rust 1.85 or newer with Cargo, and Node.js 22.12 or newer.

From the repository root, run:

```sh
./scripts/run.sh
```

Open [http://127.0.0.1:3000](http://127.0.0.1:3000). The script installs the locked Astro dependencies, builds the static site, and starts the server. Stop it with `Ctrl+C`.

The server binds only to `127.0.0.1`. By default, the SQLite file lives in the operating system's local application data directory. Set `TODO_DB_PATH=/path/to/todo.sqlite3` to choose another location, or `TODO_PORT=3001` to use a different local port.

## Development

Run `./scripts/dev.sh` from the repository root. The Astro development server is available at [http://127.0.0.1:4321](http://127.0.0.1:4321) and proxies `/api` requests to the Axum server on port 3000. The Astro server provides hot reload; stop both processes with `Ctrl+C`.

The web client builds to static assets in `web/dist`; Axum serves those assets and the REST API from the same origin during normal use.

## Agreed product shape

- One manually ordered To Do List; task titles may repeat.
- Tasks have a nonblank title, optional plain multiline notes, and a stable ID. Titles are trimmed at the edges; duplicate titles are allowed. New tasks appear at the top of Active. Completion is reversible; reopening a task restores its prior Active-list position. Active and completed tasks can be edited.
- Completed tasks remain available for review, newest first. Removed tasks stay recoverable in Trash until the user empties it; restoring a task returns its prior status and position. Trash is emptied permanently after confirmation, and its newest entries appear first. Trashed tasks must be restored before editing.
- Full-state, versioned JSON export and matching import preserve task IDs, statuses, order, and lifecycle times. Import stages an immutable server-side preview, requires confirmation, and retains every pre-import state for undo until the user deletes it. Undo restores the exact prior state, including replacing changes made after import. Invalid or unsupported files leave the current state unchanged.
- Users can reorder active tasks by dragging or with keyboard-accessible controls that move a task one position.
- The app opens to Active; new tasks start Active. Titles and notes can be edited inline in Active and Completed.
- Search uses case-insensitive partial matching across titles and notes in Active, Completed, and Trash, and labels each result with its state.
- Opening a search result preserves its state; Trash results offer restoration before editing.
- Recovery History lists retained recovery points newest first, with their date and task count. Users can preview before restoring and must confirm before deleting a recovery point. A new recovery point is created before every restore.
- A regular JSON export contains the task list only, not local Recovery History.

## Planned architecture

- Rust and Axum provide the REST API.
- SQLite stores the data in the user's application data directory by default, with a configurable location.
- The TypeScript and Astro client builds to static files served by Axum.
- The first version has no sign-in and accepts connections only from the same computer.
- Development uses the Astro dev server alongside Axum for hot reload. Normal local use starts from the source checkout with one documented command; there is no desktop installer in the first version.

## Project language and decisions

- [Domain language](../CONTEXT.md)
- [REST API contract](./api-contract.md)
- [Architecture decision records](./adr/)

## Project layout

- `CONTEXT.md` — domain glossary and language.
- `docs/` — product, API, and architecture documentation.
- `server/` — Rust/Axum REST server and SQLite persistence.
- `web/` — TypeScript/Astro browser client.
- `scripts/` — local run and development commands.
