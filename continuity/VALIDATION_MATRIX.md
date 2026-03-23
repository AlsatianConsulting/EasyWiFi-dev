# Validation Matrix

Last updated: 2026-03-23 (America/New_York)

This matrix groups `continuity/NEXT_STEPS.md` into execution classes to prioritize fast closure and isolate hardware-bound work.

Status legend:
- `todo`
- `in_progress`
- `blocked`
- `done`

## Class A — Deterministic (No RF Needed)

| Area | Representative Checks | Status | Evidence |
|---|---|---|---|
| Bookmark import/export parser matrix | CSV/JSON/JSONL/autodetect/aliases/range guards/gz suffix routing | in_progress | Regression tests in `src/ui/mod.rs` |
| Cellular playlist/export integrity | ARFCN/UARFCN/EARFCN menu/export consistency; CSV+JSON artifacts | in_progress | Export actions + tests + validator script |
| UI status/tooling guardrails | Decoder availability reasons, tooltip parity, import error messaging | in_progress | Existing test suite + manual UI checks pending |
| Artifact contract checks | `artifact_contract_version` keys + validator schema checks | in_progress | `scripts/validate_sdr_artifacts.py` |

## Class B — Host Toolchain / Hardware-Dependent (Limited RF)

| Area | Representative Checks | Status | Blocker |
|---|---|---|---|
| Decoder dry-run and fallback chain | `rtl_433`/`ADS-B`/`ACARS`/`AIS`/`POCSAG` across hardware classes | todo | Device/toolchain availability varies by host |
| Cellular scanner runtime behavior | Step/squelch/performance behavior on active hardware | todo | Requires on-device runtime checks |
| Bluetooth shortcut-to-SDR workflow | BLE/Zigbee/Thread/ISM shortcuts runtime and preset persistence | in_progress | Needs host with active BT observations |

## Class C — Live RF Validation Required

| Area | Representative Checks | Status | Evidence Needed |
|---|---|---|---|
| IQ FFT/waterfall fidelity | RTL/HackRF/bladeRF/B210 live ingest quality | todo | Multi-device capture notes |
| Satcom payload + denylist behavior | Parsed/denied/redacted state correctness in live traffic | todo | Artifact + log bundle |
| Long-run telemetry stability | `rows/map/satcom/stderr` rates over extended sessions | todo | Multi-hour run report |
| Export consumer compatibility | Downstream parser pass on real session artifacts | in_progress | Validator script + real session pack |

