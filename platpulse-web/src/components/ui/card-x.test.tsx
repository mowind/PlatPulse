import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { CardX } from './card-x'

afterEach(cleanup)

describe('CardX component', () => {
  it('renders children inside card-x-content', () => {
    const { container } = render(
      <CardX>
        <p>Card Body Content</p>
      </CardX>,
    )
    expect(screen.getByText('Card Body Content')).toBeDefined()
    const content = container.querySelector('[data-slot="card-x-content"]')
    expect(content).not.toBeNull()
    expect(content?.textContent).toBe('Card Body Content')
  })

  it('omits header when title, header, and headerExtra are all absent', () => {
    const { container } = render(<CardX>Body</CardX>)
    expect(container.querySelector('[data-slot="card-x-header"]')).toBeNull()
  })

  it('renders title in card-x-header when title is provided', () => {
    const { container } = render(<CardX title="Node Details">Body</CardX>)
    const header = container.querySelector('[data-slot="card-x-header"]')
    expect(header).not.toBeNull()
    expect(screen.getByText('Node Details')).toBeDefined()
    expect(container.querySelector('[data-slot="card-x-header-extra"]')).toBeNull()
  })

  it('renders header directly without extra wrapper when headerExtra is absent', () => {
    const { container } = render(
      <CardX header={<div data-node-region="identity" className="p-3">Identity Block</div>}>
        Body
      </CardX>,
    )
    const header = container.querySelector('[data-slot="card-x-header"]')
    expect(header).not.toBeNull()
    // Direct child of card-x-header should be the custom element
    const identityRegion = header?.querySelector('[data-node-region="identity"]')
    expect(identityRegion).not.toBeNull()
    expect(identityRegion?.parentElement).toBe(header)
    expect(container.querySelector('[data-slot="card-x-header-extra"]')).toBeNull()
  })

  it('does not render empty title container when title is null with headerExtra', () => {
    const { container } = render(
      <CardX title={null} headerExtra={<button>Action</button>}>
        Body
      </CardX>,
    )
    const header = container.querySelector('[data-slot="card-x-header"]')
    expect(header).not.toBeNull()
    expect(header?.querySelector('.truncate')).toBeNull()
    expect(screen.getByRole('button', { name: 'Action' })).toBeDefined()
  })

  it('renders headerExtra with title in two-sided alignment layout', () => {
    const { container } = render(
      <CardX
        title="Card Title"
        headerExtra={<button type="button">Refresh</button>}
      >
        Body
      </CardX>,
    )
    const header = container.querySelector('[data-slot="card-x-header"]')
    expect(header).not.toBeNull()
    expect(screen.getByText('Card Title')).toBeDefined()
    const extra = container.querySelector('[data-slot="card-x-header-extra"]')
    expect(extra).not.toBeNull()
    expect(extra?.classList.contains('ml-auto')).toBe(true)
    expect(screen.getByRole('button', { name: 'Refresh' })).toBeDefined()
  })

  it('renders headerExtra with custom header while preserving heading semantics', () => {
    const { container } = render(
      <CardX
        header={<h2 className="text-base font-semibold">Admin Panel</h2>}
        headerExtra={<span data-slot="badge">Active</span>}
      >
        Body
      </CardX>,
    )
    const heading = screen.getByRole('heading', { level: 2, name: 'Admin Panel' })
    expect(heading).toBeDefined()
    const extra = container.querySelector('[data-slot="card-x-header-extra"]')
    expect(extra).not.toBeNull()
    expect(screen.getByText('Active')).toBeDefined()
    expect(extra?.textContent).toBe('Active')
  })

  it('renders headerExtra when neither title nor header is provided', () => {
    const { container } = render(
      <CardX headerExtra={<span data-slot="status">Standalone Extra</span>}>
        Body
      </CardX>,
    )
    const header = container.querySelector('[data-slot="card-x-header"]')
    expect(header).not.toBeNull()
    const extra = container.querySelector('[data-slot="card-x-header-extra"]')
    expect(extra).not.toBeNull()
    expect(extra?.textContent).toBe('Standalone Extra')
  })

  it('renders footer when provided and supports segmented separator', () => {
    const { container } = render(
      <CardX segmented footer={<button type="button">Submit</button>}>
        Body
      </CardX>,
    )
    const footer = container.querySelector('[data-slot="card-x-footer"]')
    expect(footer).not.toBeNull()
    expect(footer?.classList.contains('border-t')).toBe(true)
    expect(screen.getByRole('button', { name: 'Submit' })).toBeDefined()
  })
})
