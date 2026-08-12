// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { filterBrowserSpec } from './generate-api.mjs'

const spec = {
  openapi: '3.1.0',
  paths: {
    '/api/agent/v1/reports': {
      post: {
        tags: ['agent'],
        operationId: 'ingestReport',
        requestBody: {
          content: {
            'application/json': {
              schema: { $ref: '#/components/schemas/AgentReport' },
            },
          },
        },
        responses: {
          '202': {
            description: 'accepted',
            content: {
              'application/json': {
                schema: { $ref: '#/components/schemas/ReportReceipt' },
              },
            },
          },
        },
      },
    },
    '/health/live': {
      get: {
        tags: ['system'],
        operationId: 'live',
        responses: {
          '200': {
            description: 'ok',
            content: {
              'application/json': {
                schema: { $ref: '#/components/schemas/LiveResponse' },
              },
            },
          },
        },
      },
    },
  },
  components: {
    schemas: {
      AgentReport: { type: 'object' },
      ReportReceipt: { type: 'object' },
      LiveResponse: { type: 'object', properties: { status: { type: 'string' } } },
    },
  },
}

describe('filterBrowserSpec', () => {
  it('drops agent operations but keeps browser operations', () => {
    const filtered = filterBrowserSpec(spec)
    expect(filtered.paths['/api/agent/v1/reports']).toBeUndefined()
    expect(filtered.paths['/health/live']).toBeDefined()
  })

  it('drops agent operations even when they share a path with browser operations', () => {
    const mixed = structuredClone(spec)
    mixed.paths['/shared'] = {
      get: {
        tags: ['system'],
        operationId: 'browserOp',
        responses: {
          '200': {
            description: 'ok',
            content: {
              'application/json': {
                schema: { $ref: '#/components/schemas/LiveResponse' },
              },
            },
          },
        },
      },
      post: {
        tags: ['agent'],
        operationId: 'agentOp',
        requestBody: {
          content: {
            'application/json': {
              schema: { $ref: '#/components/schemas/AgentReport' },
            },
          },
        },
        responses: { '202': { description: 'accepted' } },
      },
    }
    const filtered = filterBrowserSpec(mixed)
    expect(filtered.paths['/shared'].get).toBeDefined()
    expect(filtered.paths['/shared'].post).toBeUndefined()
    expect(filtered.components.schemas.AgentReport).toBeUndefined()
    expect(filtered.components.schemas.LiveResponse).toBeDefined()
  })

  it('prunes schemas only reachable from agent operations', () => {
    const filtered = filterBrowserSpec(spec)
    expect(filtered.components.schemas.AgentReport).toBeUndefined()
    expect(filtered.components.schemas.ReportReceipt).toBeUndefined()
    expect(filtered.components.schemas.LiveResponse).toBeDefined()
  })

  it('keeps schemas shared with browser operations', () => {
    const shared = structuredClone(spec)
    shared.paths['/health/ready'] = {
      get: {
        tags: ['system'],
        responses: {
          '200': {
            description: 'ready',
            content: {
              'application/json': {
                schema: { $ref: '#/components/schemas/AgentReport' },
              },
            },
          },
        },
      },
    }
    const filtered = filterBrowserSpec(shared)
    expect(filtered.components.schemas.AgentReport).toBeDefined()
  })

  it('does not mutate the input spec', () => {
    const snapshot = JSON.stringify(spec)
    filterBrowserSpec(spec)
    expect(JSON.stringify(spec)).toBe(snapshot)
  })
})
