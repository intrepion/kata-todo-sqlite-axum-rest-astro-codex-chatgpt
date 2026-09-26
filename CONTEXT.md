# To Do List

This context defines the language used for a single-person local To Do List application built as a learning kata.

## Language

**Task**:
A piece of work the user intends to do, identified by a nonblank title and optionally accompanied by plain multiline notes. Leading and trailing whitespace is removed from titles; different tasks may have the same title.
_Avoid_: Todo, item

**Task ID**:
A stable UUID identity for a task that is independent of its title and retained when the task is exported and imported.
_Avoid_: Task name

**To Do List**:
The user's single ordered collection of tasks.
_Avoid_: Project, board

**Active task**:
A task the user has not marked complete and can still work on. New tasks begin at the top of the Active list.
_Avoid_: Open item, pending task

**Completed task**:
A task the user has marked complete and can review later, with the most recently completed tasks shown first; completion can be reversed, returning the task to its prior Active-list position.
_Avoid_: Closed task

**Trash**:
A recoverable holding place for active or completed tasks removed from the To Do List. The most recently removed tasks appear first. Tasks remain there until the user empties Trash, which permanently deletes them after confirmation; restoring a task returns it to its previous status and position. A task must be restored before its title or notes can be edited.
_Avoid_: Archive, deleted task

**Task order**:
The sequence in which active tasks appear, set by the user.
_Avoid_: Sort order

**Task export**:
A portable copy of the user's active, completed, and trashed tasks that preserves their IDs, statuses, order, active-list positions, and lifecycle times. It does not include local recovery history.
_Avoid_: List dump

**Task recovery point**:
A retained copy of the full task state created before each import or recovery restore. It remains available until the user deletes it; the newest recovery points appear first.
_Avoid_: Backup snapshot

**Recovery History**:
The view where the user reviews, previews, restores, and deletes task recovery points.
_Avoid_: Import log

**Task import**:
A confirmed replacement of the To Do List from a previewed task export. The pre-import state remains available as a recovery point until the user deletes it; undo restores that exact state.
_Avoid_: Merge

**Task search**:
A case-insensitive partial-text search across task titles and notes in Active, Completed, and Trash, with each result identified by its state. Opening a result leaves its state unchanged.
_Avoid_: Find-in-list
