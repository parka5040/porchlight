//! Strict DTO shape checks, not a general JSON Schema implementation.
//!
//! Supported: local $ref, object-only allOf flattening, anyOf/oneOf, nullable
//! type arrays, required/properties/items/enum, primitive JSON types, and UUID,
//! date, RFC3339 and local-date-time string formats. Undocumented object fields
//! are always rejected (including when OpenAPI omits additionalProperties).
//! Numeric/string bounds, patterns and other format annotations are not checked.
use serde_json::{Value, json};

pub fn flattened(doc: &Value, schema: &Value) -> Value {
    if let Some(reference) = schema["$ref"].as_str() {
        return flattened(
            doc,
            doc.pointer(reference.strip_prefix('#').expect("local ref"))
                .expect("resolved ref"),
        );
    }
    if let Some(parts) = schema["allOf"].as_array() {
        let mut merged = json!({"type":"object","properties":{},"required":[]});
        for part in parts {
            let part = flattened(doc, part);
            assert_eq!(part["type"], "object", "only object allOf is supported");
            merged["properties"]
                .as_object_mut()
                .unwrap()
                .extend(part["properties"].as_object().unwrap().clone());
            if let Some(required) = part["required"].as_array() {
                merged["required"]
                    .as_array_mut()
                    .unwrap()
                    .extend(required.iter().cloned());
            }
        }
        return merged;
    }
    schema.clone()
}

pub fn matches_schema(doc: &Value, schema: &Value, value: &Value) -> bool {
    let schema = flattened(doc, schema);
    if let Some(variants) = schema["oneOf"].as_array() {
        return variants
            .iter()
            .filter(|s| matches_schema(doc, s, value))
            .count()
            == 1;
    }
    if let Some(variants) = schema["anyOf"].as_array() {
        return variants.iter().any(|s| matches_schema(doc, s, value));
    }
    let types: Vec<_> = schema["type"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_else(|| schema["type"].as_str().into_iter().collect());
    assert!(!types.is_empty(), "unsupported schema: {schema}");
    if let Some(variants) = schema["enum"].as_array()
        && !variants.contains(value)
    {
        return false;
    }
    if value.is_null() {
        return types.contains(&"null");
    }
    types.iter().any(|kind| match *kind {
        "object" => value.as_object().is_some_and(|object| {
            schema["required"].as_array().is_none_or(|keys| {
                keys.iter()
                    .all(|key| object.contains_key(key.as_str().unwrap()))
            }) && object.iter().all(|(key, value)| {
                schema["properties"]
                    .get(key)
                    .is_some_and(|s| matches_schema(doc, s, value))
            })
        }),
        "array" => value.as_array().is_some_and(|values| {
            values
                .iter()
                .all(|v| matches_schema(doc, &schema["items"], v))
        }),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "string" => value
            .as_str()
            .is_some_and(|text| match schema["format"].as_str() {
                Some("uuid") => text.parse::<uuid::Uuid>().is_ok(),
                Some("date-time") => chrono::DateTime::parse_from_rfc3339(text).is_ok(),
                Some("local-date-time") => text.parse::<chrono::NaiveDateTime>().is_ok(),
                Some("date") => text.parse::<chrono::NaiveDate>().is_ok(),
                _ => true,
            }),
        "null" => false,
        other => panic!("unsupported type: {other}"),
    })
}

pub fn conforms(doc: &Value, schema: &Value, value: &Value) {
    assert!(
        matches_schema(doc, schema, value),
        "response does not conform to schema {schema}"
    );
}

#[test]
fn nested_unions_flattening_and_formats_reject_malformed_dtos() {
    let doc = json!({"base":{"type":"object","required":["id"],"properties":{"id":{"type":"string","format":"uuid"}}}});
    let schema = json!({"anyOf":[{"type":"null"},{"allOf":[{"$ref":"#/base"},{"type":"object","required":["items"],"properties":{"items":{"type":"array","items":{"oneOf":[{"type":"string","enum":["ready"]},{"type":"integer"}]}}}}]}]});
    let valid = json!({"id":uuid::Uuid::nil(),"items":["ready",3]});
    assert!(matches_schema(&doc, &schema, &valid));
    assert!(matches_schema(&doc, &schema, &Value::Null));
    for malformed in [
        json!({"id":uuid::Uuid::nil()}),
        json!({"id":"invalid","items":[]}),
        json!({"id":uuid::Uuid::nil(),"items":[false]}),
        json!({"id":uuid::Uuid::nil(),"items":["unknown"]}),
        json!({"id":uuid::Uuid::nil(),"items":[],"extra":true}),
    ] {
        assert!(!matches_schema(&doc, &schema, &malformed));
    }
    assert!(!matches_schema(
        &doc,
        &json!({"oneOf":[{"type":"integer"},{"type":"number"}]}),
        &json!(3)
    ));
    for (format, good, bad) in [
        ("date-time", "2026-10-05T12:00:00Z", "2026-10-05T12:00:00"),
        (
            "local-date-time",
            "2026-10-05T12:00:00",
            "2026-10-05T12:00:00Z",
        ),
        ("date", "2026-10-05", "2026-10-05T12:00:00"),
    ] {
        let s = json!({"type":["string","null"],"format":format});
        assert!(matches_schema(&doc, &s, &json!(good)));
        assert!(matches_schema(&doc, &s, &Value::Null));
        assert!(!matches_schema(&doc, &s, &json!(bad)));
    }
}
