import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { Empty } from './empty'

afterEach(cleanup)

describe('Empty component', () => {
  it('renders SVG icon with valid opacity styling and without invalid opacity attribute', () => {
    const { container } = render(<Empty />)
    const svg = container.querySelector('svg')
    expect(svg).not.toBeNull()

    // Assert that the invalid SVG opacity="60" attribute is absent
    expect(svg?.hasAttribute('opacity')).toBe(false)

    // Assert that the SVG has the semi-transparent utility class opacity-60
    expect(svg?.classList.contains('opacity-60')).toBe(true)

    // In both light and dark modes, stroke inherits currentColor from text-muted-foreground
    expect(container.firstElementChild?.classList.contains('text-muted-foreground')).toBe(true)
    expect(svg?.getAttribute('stroke')).toBe('currentColor')
  })

  it('renders description when provided and omits paragraph when absent', () => {
    const { container, rerender } = render(<Empty />)
    expect(container.querySelector('p')).toBeNull()

    rerender(<Empty description="No resources available" />)
    expect(screen.getByText('No resources available')).toBeDefined()
    expect(container.querySelector('p')?.textContent).toBe('No resources available')
  })

  it('renders children and preserves custom className', () => {
    const { container } = render(
      <Empty className="test-empty-class">
        <button type="button">Action</button>
      </Empty>,
    )
    expect(container.firstElementChild?.classList.contains('test-empty-class')).toBe(true)
    expect(screen.getByRole('button', { name: 'Action' })).toBeDefined()
  })
})
