import { describe, expect, it } from 'vitest'
import { editRoundView } from './editRound'

type Round = { name: string }

const same = (left: Round, right: Round) => left.name === right.name
const holdsDraft = (current: Round, draft: Round) => current.name === draft.name.trim()

describe('editRoundView', () => {
  it('reports nothing while no round is open', () => {
    expect(
      editRoundView({
        edit: null,
        current: { name: 'Server' },
        submitting: false,
        same,
        serverHoldsDraft: holdsDraft,
      }),
    ).toEqual({ serverMoved: false, serverHasDraft: false })
  })

  it('does not call a round moved while its own save is in flight', () => {
    expect(
      editRoundView({
        edit: { draft: { name: 'Draft' }, base: { name: 'Server' } },
        current: { name: 'Server moved meanwhile' },
        submitting: true,
        same,
        serverHoldsDraft: holdsDraft,
      }),
    ).toEqual({ serverMoved: false, serverHasDraft: false })
  })

  it('reports a draft against the unchanged Server value as neither moved nor held', () => {
    expect(
      editRoundView({
        edit: { draft: { name: 'Draft' }, base: { name: 'Server' } },
        current: { name: 'Server' },
        submitting: false,
        same,
        serverHoldsDraft: holdsDraft,
      }),
    ).toEqual({ serverMoved: false, serverHasDraft: false })
  })

  it('reports the Server value that moved, with a draft it does not hold', () => {
    expect(
      editRoundView({
        edit: { draft: { name: 'Our draft' }, base: { name: 'Server' } },
        current: { name: 'Another Owner' },
        submitting: false,
        same,
        serverHoldsDraft: holdsDraft,
      }),
    ).toEqual({ serverMoved: true, serverHasDraft: false })
  })

  it('reports the Server value that already holds the draft after normalizing it', () => {
    expect(
      editRoundView({
        edit: { draft: { name: '  Another Owner  ' }, base: { name: 'Server' } },
        current: { name: 'Another Owner' },
        submitting: false,
        same,
        serverHoldsDraft: holdsDraft,
      }),
    ).toEqual({ serverMoved: true, serverHasDraft: true })
  })

  it('only asks whether the Server holds the draft once the Server moved', () => {
    let asked = false
    const view = editRoundView({
      edit: { draft: { name: 'Draft' }, base: { name: 'Server' } },
      current: { name: 'Server' },
      submitting: false,
      same,
      serverHoldsDraft: () => {
        asked = true
        return false
      },
    })
    expect(view).toEqual({ serverMoved: false, serverHasDraft: false })
    expect(asked).toBe(false)
  })
})
