import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { createRef } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { Checkbox, Radio } from './input'

afterEach(cleanup)

describe('native Emerald selection primitives', () => {
  it.each([['checkbox', Checkbox], ['radio', Radio]] as const)(
    'keeps %s input hit sizing separate from its decorative indicator', (role, Control) => {
      const ref = createRef<HTMLInputElement>()
      render(<Control ref={ref} aria-label="Selection" className="test-native" />)
      const input = screen.getByRole<HTMLInputElement>(role, { name: 'Selection' })
      expect(ref.current).toBe(input)
      expect(input.tagName).toBe('INPUT')
      expect(input.type).toBe(role)
      expect(input.classList.contains('size-11')).toBe(true)
      expect(input.classList.contains('min-w-11')).toBe(true)
      expect(input.classList.contains('min-h-11')).toBe(true)
      expect(input.classList.contains('test-native')).toBe(true)
      expect(input.classList.contains('forced-colors:opacity-100')).toBe(true)
      expect(input.tabIndex).toBe(0)
      input.focus()
      expect(document.activeElement).toBe(input)
      const indicator = input.nextElementSibling
      expect(indicator?.getAttribute('data-slot')).toBe('selection-indicator')
      expect(indicator?.getAttribute('aria-hidden')).toBe('true')
      for (const token of ['size-4', 'pointer-events-none', 'peer-focus-visible:ring-[3px]', 'peer-disabled:opacity-50', 'forced-colors:hidden']) {
        expect(indicator?.classList.contains(token)).toBe(true)
      }
      // JSDOM has no layout engine: actual 44px/16px rects and keyboard default
      // actions are exercised in browser coverage, not fabricated here.
    },
  )

  it('preserves uncontrolled checkbox label activation, submission, and reset', () => {
    const onChange = vi.fn()
    render(<form aria-label="Preferences">
      <label htmlFor="confirm">Confirm</label>
      <Checkbox id="confirm" name="confirm" value="yes" defaultChecked onChange={onChange} required aria-describedby="help" />
      <p id="help">Confirm this change.</p>
    </form>)
    const input = screen.getByRole<HTMLInputElement>('checkbox', { name: 'Confirm' })
    const form = screen.getByRole<HTMLFormElement>('form')
    expect(input.checked).toBe(true)
    expect(new FormData(form).get('confirm')).toBe('yes')
    expect(input.getAttribute('aria-describedby')).toBe('help')
    fireEvent.click(screen.getByText('Confirm', { selector: 'label' }))
    expect(input.checked).toBe(false)
    expect(onChange).toHaveBeenCalledOnce()
    expect(new FormData(form).has('confirm')).toBe(false)
    expect(input.validity.valueMissing).toBe(true)
    form.reset()
    expect(input.checked).toBe(true)
  })

  it.each([['checkbox', Checkbox], ['radio', Radio]] as const)(
    'keeps controlled %s state owned by the caller', (role, Control) => {
      const onChange = vi.fn()
      const { rerender } = render(<Control aria-label="Selection" checked={false} onChange={onChange} />)
      const input = screen.getByRole<HTMLInputElement>(role)
      fireEvent.click(input)
      expect(onChange).toHaveBeenCalledOnce()
      expect(input.checked).toBe(false)
      rerender(<Control aria-label="Selection" checked onChange={onChange} />)
      expect(input.checked).toBe(true)
    },
  )

  it.each([['checkbox', Checkbox], ['radio', Radio]] as const)(
    'preserves native disabled and disabled-fieldset %s behavior', (role, Control) => {
      const onChange = vi.fn()
      render(<form aria-label="Disabled preferences">
        <Control aria-label="Disabled" name="disabled" disabled defaultChecked onChange={onChange} />
        <fieldset disabled><Control aria-label="Fieldset disabled" name="fieldset" defaultChecked onChange={onChange} /></fieldset>
      </form>)
      for (const input of screen.getAllByRole<HTMLInputElement>(role)) {
        expect(input.matches(':disabled')).toBe(true)
        input.click()
        expect(input.checked).toBe(true)
        input.focus()
        expect(document.activeElement).not.toBe(input)
      }
      expect(onChange).not.toHaveBeenCalled()
      expect(Array.from(new FormData(screen.getByRole<HTMLFormElement>('form')))).toEqual([])
    },
  )

  it('preserves native radio grouping, form values, and reset', () => {
    render(<form aria-label="Provider">
      <label><Radio name="provider" value="local" defaultChecked />Local</label>
      <label><Radio name="provider" value="external" />External</label>
    </form>)
    const local = screen.getByRole<HTMLInputElement>('radio', { name: 'Local' })
    const external = screen.getByRole<HTMLInputElement>('radio', { name: 'External' })
    const form = screen.getByRole<HTMLFormElement>('form')
    fireEvent.click(external)
    expect(local.checked).toBe(false)
    expect(external.checked).toBe(true)
    expect(new FormData(form).getAll('provider')).toEqual(['external'])
    form.reset()
    expect(local.checked).toBe(true)
    expect(external.checked).toBe(false)
  })

  it('allows refs to set native indeterminate checkbox state', () => {
    const ref = createRef<HTMLInputElement>()
    render(<Checkbox ref={ref} aria-label="Mixed selection" />)
    const input = screen.getByRole<HTMLInputElement>('checkbox')
    input.indeterminate = true
    expect(ref.current?.indeterminate).toBe(true)
    expect(input.nextElementSibling?.classList.contains('peer-indeterminate:[&>span]:visible')).toBe(true)
    fireEvent.click(input)
    expect(input.indeterminate).toBe(false)
    expect(input.checked).toBe(true)
  })
})
