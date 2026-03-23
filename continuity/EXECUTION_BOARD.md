# Execution Board

Last updated: 2026-03-23 (America/New_York)

This board operationalizes `continuity/NEXT_STEPS.md` into an execution order with status and evidence tracking.

Status legend:
- `todo`
- `in_progress`
- `blocked`
- `done`

## Timetable

### Week 1 (2026-03-23 to 2026-03-29): Stabilization + Validation Harness
- [in_progress] Build grouped validation matrix from Priority 3 items.
- [in_progress] Close deterministic/non-RF checks first (imports, presets, exports, messages/tooltips).
- [todo] Produce baseline end-to-end artifact pack and archive paths in this board.

### Week 2 (2026-03-30 to 2026-04-05): Decoder + Cross-Hardware Validation
- [todo] Validate decoder launch/fallback chain for `rtl_433`, `ADS-B`, `ACARS`, `AIS`, `POCSAG`.
- [todo] Validate cellular ARFCN playlists and scanner presets on target hardware classes.
- [todo] Tune scan defaults where live behavior indicates poor coverage/performance.

### Week 3 (2026-04-06 to 2026-04-12): Live RF + Export Interop
- [todo] Run long-session SDR validation and verify telemetry counters.
- [todo] Validate satcom payload modes/denylist behaviors in live runs.
- [todo] Validate downstream parser compatibility for all SDR exports.

### Week 4 (2026-04-13 to 2026-04-19): Completion Gate + Next-Phase Prep
- [todo] Run full operator smoke workflow.
- [todo] Ship release-candidate hardening fixes from smoke findings.
- [todo] Finalize implementation-ready specs for multi-SDR and deeper control roadmap.

## Active Task List

## A) Validation + Quality Gates

| ID | Task | Status | Evidence / Notes |
|---|---|---|---|
| A1 | Maintain grouped validation matrix | in_progress | Added `continuity/VALIDATION_MATRIX.md` seeded from `NEXT_STEPS.md` |
| A2 | Extend automated tests for cellular/ARFCN exports and scanner coverage | in_progress | Added ARFCN export assertions for LTE 66/71 |
| A3 | Keep per-change verification (`fmt`, `check`, `test`) | in_progress | Required for every commit |
| A4 | Keep defect ledger with severity + ETA | in_progress | Initialized `continuity/DEFECT_LEDGER.md` |

## B) SDR Workflow Reliability

| ID | Task | Status | Evidence / Notes |
|---|---|---|---|
| B1 | Validate decoder fallback chains by hardware | todo | Ordered: `rtl_433`, `ADS-B`, `ACARS`, `AIS`, `POCSAG` |
| B2 | Validate scanner behavior (coverage/step/squelch) | in_progress | Cellular + ISM scanner presets expanded |
| B3 | Validate bookmark import matrix (CSV/JSON/JSONL/URL/gz) | in_progress | Parser support implemented; live matrix pending |
| B4 | Validate right-click and bookmark decode parity | in_progress | Auto-fallback added; multi-device live checks pending |

## C) Export + Interop

| ID | Task | Status | Evidence / Notes |
|---|---|---|---|
| C1 | Validate SDR export artifacts in downstream tools | in_progress | Added `scripts/validate_sdr_artifacts.py` baseline checker |
| C2 | Validate Local/Zulu timestamp behavior across artifacts | in_progress | Added `--time-mode local|zulu` checks to validator script |
| C3 | Validate cellular ARFCN CSV artifact schema/content | in_progress | Export action + regression test in place |
| C4 | Freeze artifact contract version | in_progress | Added `artifact_contract_version` to satcom summary and health snapshot JSON |

## D) Next-Phase Architecture Prep

| ID | Task | Status | Evidence / Notes |
|---|---|---|---|
| D1 | Draft multi-SDR runtime model | todo | Post-validation phase |
| D2 | Define staged deep-control milestones | todo | gqrx/HAVOC-like path |
| D3 | Define acceptance gates + telemetry for phase-2 | todo | Tied to D1/D2 |

## Blockers

- None currently logged.
