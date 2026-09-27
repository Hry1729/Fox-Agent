/**
 * Single point of import for `border-beam`.
 *
 * The library derives a per-module-instance hash for its `<style>` rules and its
 * `data-beam` attribute. Importing it from two chunks (the entry and the lazily
 * loaded settings pages) put two instances in the bundle: only one injected the
 * stylesheet, so the other instance's element had a hash no rule matched and its
 * beam rendered as an empty box. Routing both importers through this module keeps
 * a single instance (and therefore a single hash) in the graph.
 */
export { BorderBeam } from 'border-beam'
