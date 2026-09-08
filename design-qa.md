# Knowledge picker — option 2

final result: passed

## Scope and evidence

- Source visual truth: `C:/Users/hury0424/.codex/generated_images/01a04a87-32b5-7cf3-abb2-033a04893944/exec-4e454b23-d7f7-48d7-8fa7-8c78e9a2ce45.png` (second displayed option).
- Implementation: `output/knowledge-picker-qa/desktop.png`, 1280 × 720 pixels, default browser viewport 1280 × 720 CSS px; screenshot output density 1.
- Narrow implementation: `output/knowledge-picker-qa/narrow.png`, 390 × 700 pixels / CSS px.
- Source: 1358 × 1159 pixels, containing an enlarged modal approximately 1108 × 960 pixels. Source has no authoritative device density; compare the modal content at a common 600px width, not the surrounding canvas. Implementation modal measured 600 × 561 CSS px.
- State: light, local selected, ceshi checked, Fox guide disabled, remote disconnected, selection count 1.
- Real application entry at `http://127.0.0.1:1421/` opens the changed dialog from the composer menu. Populated-state evidence uses the same exported production component at `/tests/fixtures/knowledge-picker.html` with isolated fixture data; the fixture is not a production entry.

## Findings

No actionable P0/P1/P2 visual findings in the paired source/implementation review. Both images were emitted together in the same comparison tool result after the component finished loading. The initial post-reload empty capture was discarded and replaced after checking the rendered dialog.

- Typography: HarmonyOS Sans SC incumbent family; measured library names 16px, descriptions/status/body/buttons 14px, title uses 18px section token. Uses application scale tokens. Descriptions wrap instead of truncating to a tiny single line.
- Layout: same hierarchy (header, source tabs, search, grouped rows, hint, footer); 600px modal, 28px padding, 20px internal separation. Slightly taller than the normalized illustrative target to accommodate incumbent line-height and accessible controls; accepted product-system adaptation.
- Colors: incumbent Fox blue/soft-blue and neutral tokens, with visible selected row and readable unavailable row. Uses a warning icon rather than relying only on the target's amber dot.
- Assets: no raster assets required in the component. Existing Lucide and shared dialog Remix icons; native semantic checkboxes, not rasterized mock UI. Native checkbox shape and keyboard focus ring intentionally retained.
- Copy: matches the selected concept; loading, no-results, unavailable, remote failure and retry states add only necessary operational copy. No invented production records.
- Focused evidence: component text and controls were readable in the paired full-frame input; additionally inspected computed typography, dialog bounds and button bounds. No extra focused crop needed.

## Interaction checks

- Search/no-results and clearing restores list.
- Remote connection error is scoped to the remote pane, local pane remains usable.
- Retry callback switches the fixture to a available remote list.
- Cross-source selection persists; selecting a remote item then returning local reports 2.
- Save callback receives both selected references; fixture reports saved 2.
- Cancel discards an unchecked draft; reopening restores saved ceshi selection.
- Unindexed local item is disabled; already-bound unavailable entries remain visible and removable.
- Narrow viewport has no horizontal overflow (scrollWidth = innerWidth = 390), footer buttons remain inside dialog and 40px high.
- Browser console errors/warnings: none in fixture.
- TypeScript `tsc --noEmit`: passed.
- 16 tests / 53 assertions passed across picker state, binding v2 compatibility and local gateway suites. Initial restricted-run EPERM was resolved by running the same tests outside the restricted sandbox; no tests or thresholds changed.
- Mechanical design scan: only pre-existing warnings outside the changed picker selectors (animation and side-border rules), left untouched.
- Scoped `git diff --check`: passed.

## Comparison history and remaining limits

One completed paired visual comparison, no visual corrective loop required. Source/preview density and surrounding canvas differences were excluded from the visual judgment.

Desktop host persistence and live Yuxi recovery are not verified here: browser preview has no native knowledge host. No production bindings were altered during QA. Dark theme, very large catalogs and enlarged text-scale runtime states were not separately exercised.

## Implementation checklist

- [x] Source switching, search, readable rows and selection count.
- [x] Separate remote load/error from save busy/error.
- [x] Preserve selected references across sources and catalog gaps.
- [x] Use existing app tokens and preserve unrelated WIP.
- [x] Browser-rendered desktop/narrow checks and relevant tests.

## Follow-up polish

None required for this scoped change.
