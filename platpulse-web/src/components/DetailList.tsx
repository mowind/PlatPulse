/**
 * Emerald detail grid (design §15.4 and the Owner surfaces it backs; webui.md
 * §15): a label above its value, stacked on narrow viewports. The Admin
 * key/value list owns the rendering, so every Owner surface renders the same
 * detail rows and they cannot drift apart.
 */
export { AdminKeyValueList as DetailList, AdminKeyValue as DetailItem } from './AdminPage'
