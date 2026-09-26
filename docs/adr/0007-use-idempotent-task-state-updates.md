# Use idempotent task state updates

REST operations that change a task's completion state will set the requested state explicitly instead of toggling it. Repeating a request after a retry therefore leaves the task in the same state rather than reversing the prior change.
