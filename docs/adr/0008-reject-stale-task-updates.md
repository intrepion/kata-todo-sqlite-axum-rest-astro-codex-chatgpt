# Reject stale task updates

Task responses will include an ETag and task writes will send it in `If-Match`. Reordering and Trash-wide writes will use a collection ETag; imports and recovery restores will require the current To Do List ETag. The server will reject writes based on an older revision without changing data. This avoids silently losing edits when the same person has the application open in multiple tabs or confirms a preview after the list changes.
