# Retain import recovery points until deleted

The application will retain the state being replaced before each import or recovery restore until the user deletes it, with no automatic expiration. This favors recoverability for small task snapshots and makes every full-state restore reversible; users control how long older copies remain.
