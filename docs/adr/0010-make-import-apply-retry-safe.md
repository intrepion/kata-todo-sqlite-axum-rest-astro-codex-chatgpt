# Make import apply retry-safe

An import preview may be applied only once. Retrying its apply request returns the original outcome instead of replacing the list again or creating another recovery point, avoiding duplicate side effects when a response is lost.
