import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, expect, it } from 'vitest'
import { Tabs, TabsList, TabsTrigger } from './tabs'
afterEach(cleanup)
it('styles Radix state and orientation, not Reka attributes', () => {
  render(<Tabs defaultValue="a" orientation="vertical"><TabsList><TabsTrigger value="a">Alpha</TabsTrigger><TabsTrigger value="b">Beta</TabsTrigger></TabsList></Tabs>)
  expect(screen.getByRole('tab', { name: 'Alpha' }).getAttribute('data-state')).toBe('active')
  expect(screen.getByRole('tab', { name: 'Alpha' }).className).toContain('data-[state=active]:bg-background')
  expect(screen.getByRole('tablist').className).toContain('group-data-[orientation=vertical]/tabs:flex-col')
})

it('vertical Radix keyboard navigation skips disabled triggers and moves selection with focus', async () => {
  render(<Tabs defaultValue="a" orientation="vertical"><TabsList><TabsTrigger value="a">Alpha</TabsTrigger><TabsTrigger value="disabled" disabled>Unavailable</TabsTrigger><TabsTrigger value="b">Beta</TabsTrigger></TabsList></Tabs>)
  const first = screen.getByRole('tab', { name: 'Alpha' })
  const second = screen.getByRole('tab', { name: 'Beta' })
  first.focus()
  fireEvent.keyDown(first, { key: 'ArrowDown' })
  await waitFor(() => expect(document.activeElement).toBe(second))
  expect(second.getAttribute('data-state')).toBe('active')
  fireEvent.keyDown(second, { key: 'Home' })
  await waitFor(() => expect(document.activeElement).toBe(first))
  expect(first.getAttribute('data-state')).toBe('active')
})
