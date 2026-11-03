# Archived research

These notes are frozen. Each was written while its issue was open and records what the code looked like then: `file.rs:line` references point at that commit, not at `main`. Do not update them, and do not cite them as the current contract.

The current contract lives in `docs/design/`, `docs/adr/` and `CONTEXT.md`. Read an archived note only for the reasoning behind a decision those documents already state.

## Lifecycle of a research note

1. Write findings for an open issue under `docs/research/`.
2. When the issue closes, move each conclusion that still holds into the design document, an ADR, or `CONTEXT.md`, in the pull request that closes the issue.
3. In the same pull request, move the note to `docs/research/archive/` and point any remaining links at the new path.

`docs/research/` itself keeps only notes that describe current external facts (Provider APIs, licences, metric samples, upstream bugs) and are still cited from code or design documents.
