# WirelessExplorer Continuity Bundle

This folder is the handoff package for moving development to a new Ubuntu system.

## What this contains

1. `INITIAL_REQUIREMENTS.md`
   - Condensed version of the first product requirements.
2. `UPDATED_REQUIREMENTS.md`
   - Major requirement changes and scope updates that happened after the initial request.
3. `CURRENT_STATUS.md`
   - What is implemented now, what is partial, what is still missing.
4. `NEXT_STEPS.md`
   - Prioritized implementation and validation backlog.
5. `DEPENDENCIES.md`
   - Build dependencies, runtime tools, optional decoder tools, and hardware notes.
6. `UBUNTU_BOOTSTRAP.md`
   - Fresh Ubuntu setup procedure.
7. `REPO_MAP.md`
   - Module map and key entry points.
8. `LAST_SESSION.md`
   - Exact state of the project at the point this continuity bundle was created.
9. `POLICY_RESTRICTIONS.md`
   - Features explicitly declined due to policy restrictions.
10. `transfer/`
   - Scripts and instructions for creating and restoring transfer artifacts.
11. `bootstrap_ubuntu.sh`
   - Fresh-machine bootstrap script.

## Fastest restore path on a new machine

1. Copy `continuity/transfer/WirelessExplorer.bundle` and this `continuity/` folder to the new box.
2. Clone from the bundle:
   - `git clone WirelessExplorer.bundle WirelessExplorer`
3. Enter the repo:
   - `cd WirelessExplorer`
4. Run bootstrap:
   - `bash continuity/bootstrap_ubuntu.sh`
5. Validate:
   - `cargo test -q`
   - `cargo build -q`
6. Run:
   - `cargo run`
   - or `sudo -n ./target/debug/wirelessexplorer`

## Project identity

- Current project name: `WirelessExplorer`
- Previous project name during development: `SimpleSTG`
- Primary binary: `wirelessexplorer`
- Privileged helper binary: `wirelessexplorer-helper`
- Legacy helper still present for compatibility: `src/bin/simplestg-helper.rs`

## Important baseline constraints

1. The app is intended to remain **100% passive**.
2. Packet-inspection features for IP/content analysis were later removed from scope.
3. Current focus areas are:
   - Wi-Fi passive collection and export
   - Bluetooth passive collection and metadata/enumeration support
   - SDR spectrum/decoder integration
   - multi-adapter support where practical
