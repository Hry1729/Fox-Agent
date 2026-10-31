/// <reference types="vite/client" />

// Compile-time constants injected by vite.config.ts `define` (no .env needed).
declare const __FOX_PROFILING__: boolean
/** Build-time `FOX_DEV_UI=1`: developer UI on by default (see features/chat/developer-ui.ts). */
declare const __FOX_DEV_UI__: boolean
declare const __FOX_BUILD_ID__: string
