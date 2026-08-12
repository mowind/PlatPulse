import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import App from './App'

afterEach(cleanup)

describe('App shell', () => {
  it('renders the Home shell with reachable Admin navigation', () => {
    render(<App />)
    expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Admin' })).toBeTruthy()
  })

  it('navigates Home → Admin → Home without leaving the shell', () => {
    render(<App />)
    fireEvent.click(screen.getByRole('link', { name: 'Admin' }))
    expect(screen.getByRole('heading', { level: 1, name: 'Admin' })).toBeTruthy()
    fireEvent.click(screen.getByRole('link', { name: 'Home' }))
    expect(screen.getByRole('heading', { level: 1, name: 'Home' })).toBeTruthy()
  })
})
