/** IconPark Outline artwork (Apache-2.0), as selected by Emerald Header.vue.
 * https://github.com/Tokinx/komari-theme-emerald/blob/master/src/components/Header.vue
 * https://github.com/bytedance/IconPark
 * Bundled locally: rendering never depends on Iconify CDN availability.
 */
const icons = {
  'dark-mode': (<g fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeMiterlimit="10" strokeWidth="4"><path d="m24.003 4l5.27 5.27h9.457v9.456l5.27 5.27l-5.27 5.278v9.456h-9.456L24.004 44l-5.278-5.27H9.27v-9.456L4 23.997l5.27-5.27V9.27h9.456z"/><path d="M27 17c0 8-5 9-10 9c0 4 6.5 8 12 4s2-13-2-13"/></g>),
  'sun-one': (<g fill="none"><path stroke="currentColor" strokeLinejoin="round" strokeWidth="4" d="M24 37c7.18 0 13-5.82 13-13s-5.82-13-13-13s-13 5.82-13 13s5.82 13 13 13Z"/><path fill="currentColor" d="M24 6a2.5 2.5 0 1 0 0-5a2.5 2.5 0 0 0 0 5m14.5 6a2.5 2.5 0 1 0 0-5a2.5 2.5 0 0 0 0 5m6 14.5a2.5 2.5 0 1 0 0-5a2.5 2.5 0 0 0 0 5m-6 14.5a2.5 2.5 0 1 0 0-5a2.5 2.5 0 0 0 0 5M24 47a2.5 2.5 0 1 0 0-5a2.5 2.5 0 0 0 0 5M9.5 41a2.5 2.5 0 1 0 0-5a2.5 2.5 0 0 0 0 5m-6-14.5a2.5 2.5 0 1 0 0-5a2.5 2.5 0 0 0 0 5m6-14.5a2.5 2.5 0 1 0 0-5a2.5 2.5 0 0 0 0 5"/></g>),
  'moon': (<path fill="none" stroke="currentColor" strokeLinejoin="round" strokeWidth="4" d="M28.053 4.41c-5.47 1.427-9.507 6.4-9.507 12.317c0 7.03 5.698 12.728 12.727 12.728c5.916 0 10.89-4.038 12.316-9.508A20 20 0 0 1 44 24c0 11.046-8.954 20-20 20S4 35.046 4 24S12.954 4 24 4c1.389 0 2.744.141 4.053.41Z"/>),
  'setting': (<g fill="none" stroke="currentColor" strokeLinejoin="round" strokeWidth="4"><path d="M36.686 15.171a15.4 15.4 0 0 1 2.529 6.102H44v5.454h-4.785a15.4 15.4 0 0 1-2.529 6.102l3.385 3.385l-3.857 3.857l-3.385-3.385a15.4 15.4 0 0 1-6.102 2.529V44h-5.454v-4.785a15.4 15.4 0 0 1-6.102-2.529l-3.385 3.385l-3.857-3.857l3.385-3.385a15.4 15.4 0 0 1-2.529-6.102H4v-5.454h4.785a15.4 15.4 0 0 1 2.529-6.102l-3.385-3.385l3.857-3.857l3.385 3.385a15.4 15.4 0 0 1 6.102-2.529V4h5.454v4.785a15.4 15.4 0 0 1 6.102 2.529l3.385-3.385l3.857 3.857z"/><path d="M24 29a5 5 0 1 0 0-10a5 5 0 0 0 0 10Z"/></g>),
}

export function EmeraldActionIcon({ name }: { name: keyof typeof icons }) {
  return <svg className="emerald-action-icon" width="18" height="18" viewBox="0 0 48 48" data-icon={name} aria-hidden="true" focusable="false">{icons[name]}</svg>
}
