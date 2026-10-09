/**
 * Register a DOM before any test module is evaluated.
 *
 * Two third-party modules capture a module-level decision at *first import*:
 *
 *  - `react-dom/client` freezes `canUseDOM` and `isInputEventSupported` when it is first
 *    evaluated. If that happens while `window`/`document` are undefined, the input event
 *    polyfill later dereferences a module-level `activeElementInst` that is `null` and throws
 *    inside `extractEvents`, so React's `onKeyDown` never runs.
 *  - `@radix-ui/react-use-layout-effect` freezes
 *    `globalThis?.document ? React.useLayoutEffect : () => {}`. Frozen as a no-op, every
 *    Radix Portal stays unmounted, so an opened menu renders `null` with no error.
 *
 * Both are correct in the real app, where the document exists before any bundle loads; only
 * the test process can get the order wrong. A per-file `if (!GlobalRegistrator.isRegistered)`
 * guard cannot help, because the damage is done during that same file's static imports —
 * which are evaluated before any statement in the file body. Registering once here, in a
 * preload, is the only point that precedes every module's static imports.
 *
 * No test file unregisters, so this registration lasts for the whole test process.
 */
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true
