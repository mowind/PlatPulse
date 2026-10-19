import { describe, expect, it } from 'vitest'
import { ADMIN_NODES_RETURN_KEY, adminNodesDeparture, readAdminNodesReturn } from './adminNodesReturn'

describe('Admin Nodes list return', () => {
  it('returns to the list reading it was left from, filters and sort in their order', () => {
    const state = adminNodesDeparture('?health=unhealthy&network=platon-e2e&sort=name')
    expect(readAdminNodesReturn(state)).toBe('/admin/nodes?health=unhealthy&network=platon-e2e&sort=name')
    expect(readAdminNodesReturn(adminNodesDeparture(''))).toBe('/admin/nodes')
    expect(readAdminNodesReturn(adminNodesDeparture('?visibility=private&lifecycle=retired'))).toBe(
      '/admin/nodes?visibility=private&lifecycle=retired',
    )
  })

  it('keeps only the list parameters, so a detail section or anything else never rides along', () => {
    expect(readAdminNodesReturn(adminNodesDeparture('?section=manage&sort=network&return=x&health=unknown'))).toBe(
      '/admin/nodes?sort=network&health=unknown',
    )
    // One value per parameter, as the list reads it.
    expect(readAdminNodesReturn(adminNodesDeparture('?sort=name&sort=network'))).toBe('/admin/nodes?sort=name')
  })

  it('never takes a location from the state: the path is always the Nodes list', () => {
    for (const search of ['//evil.example/', 'https://evil.example/?sort=name', '/admin/agents?sort=name', '?next=/admin/settings']) {
      const href = readAdminNodesReturn({ [ADMIN_NODES_RETURN_KEY]: { search } })
      expect(href?.startsWith('/admin/nodes')).toBe(true)
      expect(href).not.toMatch(/evil|agents|settings/)
    }
    expect(readAdminNodesReturn({ [ADMIN_NODES_RETURN_KEY]: { search: '?sort=name&' + 'x'.repeat(5000) } })).toBe('/admin/nodes')
    expect(readAdminNodesReturn({ [ADMIN_NODES_RETURN_KEY]: { search: '?network=' + 'n'.repeat(300) } })).toBe('/admin/nodes')
  })

  it('reads anything that is not a departure as no return at all', () => {
    for (const state of [null, undefined, 'x', 1, {}, { from: '/admin' }, { [ADMIN_NODES_RETURN_KEY]: null }, { [ADMIN_NODES_RETURN_KEY]: { search: 3 } }]) {
      expect(readAdminNodesReturn(state)).toBeNull()
    }
  })
})
