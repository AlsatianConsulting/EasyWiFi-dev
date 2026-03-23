#!/usr/bin/env python3
"""Validate WirelessExplorer SDR export artifacts for downstream compatibility."""

from __future__ import annotations

import argparse
import csv
import json
import sys
from pathlib import Path
from typing import Iterable


def expect_file(path: Path, errors: list[str]) -> None:
    if not path.exists():
        errors.append(f"missing file: {path}")
    elif not path.is_file():
        errors.append(f"not a file: {path}")


def expect_csv_columns(path: Path, required: Iterable[str], errors: list[str]) -> None:
    expect_file(path, errors)
    if errors:
        return
    try:
        with path.open("r", encoding="utf-8", newline="") as handle:
            reader = csv.reader(handle)
            header = next(reader, None)
    except Exception as exc:  # pragma: no cover - defensive
        errors.append(f"failed to read CSV header {path}: {exc}")
        return
    if not header:
        errors.append(f"empty CSV header: {path}")
        return
    missing = [column for column in required if column not in header]
    if missing:
        errors.append(f"CSV {path} missing columns: {', '.join(missing)}")


def load_json(path: Path, errors: list[str]):
    expect_file(path, errors)
    if errors:
        return None
    try:
        with path.open("r", encoding="utf-8") as handle:
            return json.load(handle)
    except Exception as exc:  # pragma: no cover - defensive
        errors.append(f"failed to parse JSON {path}: {exc}")
        return None


def expect_json_array(path: Path, required_keys: Iterable[str], errors: list[str]) -> None:
    data = load_json(path, errors)
    if data is None:
        return
    if not isinstance(data, list):
        errors.append(f"JSON {path} is not an array")
        return
    if not data:
        errors.append(f"JSON {path} array is empty")
        return
    first = data[0]
    if not isinstance(first, dict):
        errors.append(f"JSON {path} first element is not an object")
        return
    missing = [key for key in required_keys if key not in first]
    if missing:
        errors.append(f"JSON {path} first row missing keys: {', '.join(missing)}")


def timestamp_matches_mode(value: str, mode: str) -> bool:
    if mode == "any":
        return True
    cleaned = value.strip()
    if not cleaned:
        return False
    looks_zulu = cleaned.endswith("Z") or "UTC" in cleaned
    if mode == "zulu":
        return looks_zulu
    if mode == "local":
        return not looks_zulu
    return True


def expect_csv_timestamp_mode(path: Path, mode: str, errors: list[str]) -> None:
    if mode == "any":
        return
    expect_file(path, errors)
    if errors:
        return
    try:
        with path.open("r", encoding="utf-8", newline="") as handle:
            reader = csv.DictReader(handle)
            first = next(reader, None)
    except Exception as exc:  # pragma: no cover - defensive
        errors.append(f"failed to inspect CSV timestamps {path}: {exc}")
        return
    if not first:
        return
    ts = (first.get("timestamp") or "").strip()
    if not timestamp_matches_mode(ts, mode):
        errors.append(
            f"CSV {path} timestamp mode mismatch (expected {mode}, got `{ts}`)"
        )


def expect_json_timestamp_mode(path: Path, mode: str, errors: list[str]) -> None:
    if mode == "any":
        return
    data = load_json(path, errors)
    if data is None:
        return
    if isinstance(data, list):
        if not data:
            return
        row = data[0]
        if not isinstance(row, dict):
            return
        ts = str(row.get("timestamp", "")).strip()
        if ts and not timestamp_matches_mode(ts, mode):
            errors.append(
                f"JSON {path} timestamp mode mismatch (expected {mode}, got `{ts}`)"
            )
    elif isinstance(data, dict):
        ts = str(data.get("generated_at", "")).strip()
        if ts and not timestamp_matches_mode(ts, mode):
            errors.append(
                f"JSON {path} generated_at mode mismatch (expected {mode}, got `{ts}`)"
            )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Validate SDR export artifacts produced by WirelessExplorer."
    )
    parser.add_argument(
        "--session-dir",
        required=True,
        type=Path,
        help="Path to a WirelessExplorer session directory",
    )
    parser.add_argument(
        "--time-mode",
        choices=["any", "local", "zulu"],
        default="any",
        help="Expected rendered time mode in artifact timestamps",
    )
    args = parser.parse_args()

    session_dir: Path = args.session_dir
    csv_dir = session_dir / "csv"
    json_dir = session_dir / "json"

    errors: list[str] = []

    expect_csv_columns(
        csv_dir / "sdr_decode_rows.csv",
        ["timestamp", "decoder", "freq_hz", "protocol", "message", "raw"],
        errors,
    )
    expect_csv_columns(
        csv_dir / "sdr_aircraft_correlation.csv",
        ["key", "icao_hex", "callsign", "adsb_rows", "acars_rows", "total_rows"],
        errors,
    )
    expect_csv_columns(
        csv_dir / "sdr_satcom_observations.csv",
        [
            "timestamp",
            "decoder",
            "protocol",
            "freq_hz",
            "band",
            "encryption_posture",
            "payload_capture_mode",
            "payload_parse_state",
            "message",
            "raw",
        ],
        errors,
    )
    expect_csv_columns(
        csv_dir / "cellular_arfcn_playlist.csv",
        ["link", "band", "channel_type", "channel", "frequency_hz", "frequency_mhz"],
        errors,
    )
    expect_csv_timestamp_mode(csv_dir / "sdr_decode_rows.csv", args.time_mode, errors)
    expect_csv_timestamp_mode(
        csv_dir / "sdr_satcom_observations.csv", args.time_mode, errors
    )

    expect_json_array(
        json_dir / "sdr_decode_rows.json",
        ["timestamp", "decoder", "freq_hz", "protocol", "message", "raw"],
        errors,
    )
    expect_json_array(
        json_dir / "sdr_aircraft_correlation.json",
        ["key", "adsb_rows", "acars_rows", "total_rows"],
        errors,
    )
    expect_json_array(
        json_dir / "sdr_satcom_observations.json",
        ["timestamp", "decoder", "protocol", "freq_hz", "payload_parse_state"],
        errors,
    )
    expect_json_array(
        json_dir / "cellular_arfcn_playlist.json",
        ["link", "band", "channel_type", "channel", "frequency_hz", "frequency_mhz"],
        errors,
    )
    expect_json_timestamp_mode(json_dir / "sdr_decode_rows.json", args.time_mode, errors)
    expect_json_timestamp_mode(
        json_dir / "sdr_satcom_observations.json", args.time_mode, errors
    )

    summary_json = load_json(json_dir / "sdr_satcom_summary.json", errors)
    if isinstance(summary_json, dict):
        for key in ("artifact_contract_version", "generated_at", "total_rows"):
            if key not in summary_json:
                errors.append(f"JSON {json_dir / 'sdr_satcom_summary.json'} missing key: {key}")
    elif summary_json is not None:
        errors.append(f"JSON {json_dir / 'sdr_satcom_summary.json'} is not an object")
    expect_json_timestamp_mode(
        json_dir / "sdr_satcom_summary.json", args.time_mode, errors
    )

    health_json = load_json(json_dir / "sdr_health_snapshot.json", errors)
    if isinstance(health_json, dict):
        for key in (
            "artifact_contract_version",
            "generated_at",
            "decoder_telemetry",
            "aircraft_correlation_summary",
            "satcom_summary",
        ):
            if key not in health_json:
                errors.append(f"JSON {json_dir / 'sdr_health_snapshot.json'} missing key: {key}")
    elif health_json is not None:
        errors.append(f"JSON {json_dir / 'sdr_health_snapshot.json'} is not an object")
    expect_json_timestamp_mode(
        json_dir / "sdr_health_snapshot.json", args.time_mode, errors
    )

    if errors:
        print("FAIL: artifact validation failed", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(f"PASS: SDR artifact validation succeeded for {session_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
