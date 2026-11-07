"""Durable, versioned semantic schema identity for benchmark results."""

from __future__ import annotations

import hashlib
import json
from decimal import Decimal
from pathlib import Path, PurePosixPath


def canonical_registrations(manifest: dict) -> list[dict]:
    """Normalize unordered registration and index-map iteration order."""
    result = []
    for registration in manifest["registrations"]:
        entry = dict(registration)
        for key in ("indexed_paths", "custom_bindings"):
            entry[key] = sorted(entry[key])
        result.append(entry)
    return sorted(result, key=lambda entry: entry["name"])


def schema_identity(
    result_dir: Path,
    expected_collections: set[str] | None = None,
    declared_digest: str | None = None,
) -> tuple[str | None, str | None]:
    """Return the saved digest or a reason that this run is unverifiable.

    The canonical JSON is emitted from the exact documents parsed by the Rust
    benchmark loader. Its bytes retain numeric spelling/precision; the parsed
    copy is checked for completeness and consistency without re-encoding it.
    """
    path = result_dir / "schema-manifest.json"
    if not path.is_file():
        return None, "unverifiable schema baseline: missing schema-manifest.json"
    try:
        artifact = json.loads(path.read_text(), parse_float=Decimal)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        return None, f"unverifiable schema baseline: {error}"
    return validate_schema_artifact(artifact, expected_collections, declared_digest)


def validate_schema_artifact(
    artifact: object,
    expected_collections: set[str] | None = None,
    declared_digest: str | None = None,
) -> tuple[str | None, str | None]:
    try:
        if not isinstance(artifact, dict) or artifact.get("version") != 1:
            raise ValueError("unsupported schema manifest version")
        canonical = artifact["canonical_json"]
        schemas = artifact["schemas"]
        digest = artifact["sha256"]
        if not isinstance(canonical, str) or not isinstance(schemas, list):
            raise ValueError("invalid schema manifest content")
        if not isinstance(digest, str) or len(digest) != 64:
            raise ValueError("invalid schema digest")
        if hashlib.sha256(canonical.encode("utf-8")).hexdigest() != digest:
            raise ValueError("schema digest does not match saved canonical JSON")
        def sorted_object(pairs: list[tuple[str, object]]) -> dict:
            keys = [key for key, _ in pairs]
            if keys != sorted(set(keys)):
                raise ValueError("canonical schema object keys are unsorted or duplicated")
            return dict(pairs)

        parsed = json.loads(canonical, parse_float=Decimal, object_pairs_hook=sorted_object)
        if parsed != schemas:
            raise ValueError("saved schemas differ from canonical schema set")
        identities = []
        for entry in schemas:
            if not isinstance(entry, dict) or set(entry) != {"collection", "path", "schema"}:
                raise ValueError("invalid schema inventory entry")
            name, relative, schema = entry["collection"], entry["path"], entry["schema"]
            if not isinstance(name, str) or not name:
                raise ValueError("invalid collection name")
            if not isinstance(relative, str) or not relative.startswith(".objekte/collections/") or not relative.endswith(".schema.json"):
                raise ValueError("invalid relative schema path")
            if any(part in ("", ".", "..") for part in PurePosixPath(relative).parts):
                raise ValueError("unsafe relative schema path")
            if not isinstance(schema, dict) or not isinstance(schema.get("x-objekte"), dict) or schema["x-objekte"].get("name") != name:
                raise ValueError("schema collection identity differs")
            identities.append((name, relative))
        if identities != sorted(set(identities)) or len({name for name, _ in identities}) != len(identities):
            raise ValueError("duplicate or unsorted schema inventory")
        if expected_collections is not None and {name for name, _ in identities} != expected_collections:
            raise ValueError("partial or unexpected schema inventory")
        if declared_digest is not None and digest != declared_digest:
            raise ValueError("run manifest schema digest differs from artifact")
        return digest, None
    except (json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        return None, f"unverifiable schema baseline: {error}"


def compare_schema_identity(
    baseline: Path,
    current: Path,
    old_names: set[str] | None,
    new_names: set[str] | None,
    old_digest: str | None,
    new_digest: str | None,
) -> list[str]:
    old_identity, old_error = schema_identity(baseline, old_names, old_digest)
    new_identity, new_error = schema_identity(current, new_names, new_digest)
    failures = []
    if old_error:
        failures.append(f"baseline {old_error}")
    if new_error:
        failures.append(f"current {new_error}")
    if old_identity is not None and new_identity is not None and old_identity != new_identity:
        failures.append("semantic schema differs from baseline")
    return failures
