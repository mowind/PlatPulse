/**
 * Emerald detail grid (the Owner surfaces it backs, webui.md §15): a label above its value, stacked on narrow viewports. This module is
 * only the established name for the Admin key/value list in components/AdminPage.tsx,
 * which owns the rendering, so every Owner surface renders the same detail rows.
 */
export { AdminKeyValueList as DetailList, AdminKeyValue as DetailItem } from './AdminPage'
