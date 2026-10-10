#!/usr/bin/env python3
"""C6 recordings must use the canonical OpenAPI properties/enums/formats."""
import datetime
import json
from pathlib import Path
import uuid

ROOT = Path(__file__).resolve().parents[1]
SPEC = json.loads((ROOT / "docs/openapi.json").read_text())
SCHEMAS = SPEC["components"]["schemas"]


def check(value, schema):
    if "$ref" in schema:
        return check(value, SCHEMAS[schema["$ref"].split("/")[-1]])
    for keyword in ("oneOf", "anyOf"):
        if keyword in schema:
            matches = 0
            for branch in schema[keyword]:
                try:
                    check(value, branch)
                    matches += 1
                except (AssertionError, KeyError, TypeError, ValueError):
                    pass
            assert matches == 1 if keyword == "oneOf" else matches >= 1, (keyword, matches)
            return
    for branch in schema.get("allOf", []):
        check(value, branch)
    types = schema.get("type", [])
    if isinstance(types, str):
        types = [types]
    kind = "null" if value is None else "boolean" if isinstance(value, bool) else "integer" if isinstance(value, int) else "number" if isinstance(value, float) else "string" if isinstance(value, str) else "array" if isinstance(value, list) else "object"
    assert not types or kind in types or kind == "integer" and "number" in types, (kind, types)
    if value is None:
        assert "null" in schema.get("type", []) or schema.get("nullable")
        return
    if "enum" in schema:
        assert value in schema["enum"], (value, schema["enum"])
    if isinstance(value, dict):
        assert set(schema.get("required", [])) <= value.keys()
        properties = schema.get("properties", {})
        assert value.keys() <= properties.keys(), value.keys() - properties.keys()
        for key, field in value.items():
            check(field, properties[key])
    if isinstance(value, list):
        for row in value:
            check(row, schema["items"])
    if isinstance(value, str):
        if schema.get("format") == "uuid":
            uuid.UUID(value)
        if schema.get("format") == "date-time":
            assert datetime.datetime.fromisoformat(value).tzinfo is not None
        if schema.get("format") == "local-time":
            assert len(value) == 8 and datetime.time.fromisoformat(value).tzinfo is None
    if isinstance(value, int) and not isinstance(value, bool):
        assert value >= schema.get("minimum", value)
        assert value <= schema.get("maximum", value)


for name, schema in [("preview", "ImportSummary"), ("availability", "AvailabilityResponse"), ("suggestion", "SuggestionResponse"), ("feed", "Source"), ("device", "Device")]:
    fixture = ROOT / f"client/shared/src/jvmTest/resources/planning-fixtures/{name}.json"
    check(json.loads(fixture.read_text()), SCHEMAS[schema])
assert "/v1/export/member" in SPEC["paths"]
assert "/v1/admin/export/household" in SPEC["paths"]
print("C6 real-handler recordings: canonical required fields, enums and time formats OK")
