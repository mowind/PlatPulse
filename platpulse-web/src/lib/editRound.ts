/** One edit round: the draft and the Server value it started from. */
export type EditRound<Draft> = {
  draft: Draft
  base: Draft
}

/**
 * The two Server-facing facts every confirmed inline editor derives from its
 * open round, with no knowledge of the value's shape:
 *
 * - `serverMoved`: the Server value is no longer the one the round started
 *   from, so a confirmation given against the earlier value no longer stands.
 *   The editor's own save is excluded while it is in flight.
 * - `serverHasDraft`: once the Server moved, it now holds exactly what this
 *   draft would send (for example a write whose answer was lost reached it
 *   after all). Nothing is left to save.
 *
 * `same` compares the editor's value shape for the server-moved rule;
 * `serverHoldsDraft` applies that editor's own normalization and request
 * comparison.
 */
export function editRoundView<Draft>({
  edit,
  current,
  submitting,
  same,
  serverHoldsDraft,
}: {
  edit: EditRound<Draft> | null
  current: Draft
  submitting: boolean
  same: (left: Draft, right: Draft) => boolean
  serverHoldsDraft: (current: Draft, draft: Draft) => boolean
}): { serverMoved: boolean; serverHasDraft: boolean } {
  if (edit === null) return { serverMoved: false, serverHasDraft: false }
  const serverMoved = !submitting && !same(current, edit.base)
  return {
    serverMoved,
    serverHasDraft: serverMoved && serverHoldsDraft(current, edit.draft),
  }
}
