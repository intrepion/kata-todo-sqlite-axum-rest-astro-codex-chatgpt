# Use JSON for full-state portability

Task export and import use versioned JSON and preserve stable task IDs, the full task state (active, completed, or trashed), task order, and lifecycle timestamps. Active position is retained for tasks that will return to Active, including completed and trashed tasks. The current implementation accepts format version 1 and rejects unsupported versions before changing data. Regular exports contain the task list but omit local recovery history, keeping a portable list separate from device-specific recovery copies.
