# Last Session Snapshot

Generated: 2026-03-15T21:39:10+00:00

## Last completed steps before continuity packaging

1. Extended Bluetooth action routing so the selected device is controlled through the adapter/controller that actually observed it.
2. Extended Wi-Fi test mode to support multiple interfaces in one invocation.
3. Extended SDR dependency coverage for plugin-backed decoders and improved noninteractive SDR validation.
4. Continued validating filter/watchlist alignment and AP detail scoping.
5. Began packaging the repository for migration to a new development machine.

## Most recent committed history before this continuity commit

1. `e3aca97` `Add multi-interface wifi test mode`
2. `7f7fdb5` `Route bluetooth actions by observed adapter`
3. `6ee438d` `Harden Wi-Fi test mode capture fallback and BSSID filtering`
4. `b493189` `Add SDR dependency-plan regression tests`
5. `3e04058` `Improve SDR dependency install fallback and table filter/watchlist alignment`

## Latest observed validation commands

### Wi-Fi

```bash
sudo -n ./target/debug/easywifi --test-wifi --interface wlx1cbfcef8e928,wlp0s20f3 --channels 1,6,11 --duration-secs 6 --max-networks 25
```

Sample result at continuity time:

1. `wlx1cbfcef8e928` collected `Precious` on channel `11`
2. `wlp0s20f3` collected nothing in that short sample window

### Bluetooth

```bash
./target/debug/easywifi --test-bluetooth --source bluez --controller all --duration-secs 10
./target/debug/easywifi --test-bluetooth --source both --controller all --ubertooth-device all --duration-secs 10
```

Sample result at continuity time:

1. no devices collected in those short sample windows
2. code paths executed without crashing

### SDR

```bash
./target/debug/easywifi --test-sdr --duration-secs 3
```

Sample result at continuity time:

1. runtime started and stopped cleanly
2. no decode rows collected in the sample window
3. missing-tool reporting was working

## Important current repo facts

1. There is **no git remote** configured.
2. Use the git bundle path in `continuity/transfer/` to move the full repository history.
3. This repo contains a large amount of local test output and screenshots from UI validation. Those are not source and should not be treated as required migration inputs.
4. The development environment at continuity time was Ubuntu `22.04.5 LTS` on `amd64`.

## Immediate next engineering task after migration

Resume with:

1. validating the filter row placement under each header after migration
2. confirming the AP selected-detail pane only shows the selected AP's clients
3. continuing the SDR implementation backlog from `NEXT_STEPS.md`
