# Stage import previews before apply

The server will validate and stage an immutable import snapshot, return a preview, and apply that same snapshot only after explicit confirmation. A staged preview is temporary and is discarded when applied, canceled, or when the server restarts. Apply also checks that the current To Do List ETag has not changed since preview; a stale preview must be refreshed and confirmed again. This binds the confirmed action to both the data the user reviewed and the list state being replaced.
