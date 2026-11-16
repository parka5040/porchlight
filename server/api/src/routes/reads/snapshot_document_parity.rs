use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use std::collections::HashSet;
struct Unique;
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Check;
        impl<'de> Visitor<'de> for Check {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Unique, M::Error> {
                let mut keys = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    map.next_value::<Unique>()?;
                }
                Ok(Unique)
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Unique, S::Error> {
                while seq.next_element::<Unique>()?.is_some() {}
                Ok(Unique)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Unique, E> {
                Ok(Unique)
            }
        }
        d.deserialize_any(Check)
    }
}
pub(super) fn unique(bytes: &[u8]) -> Result<(), serde_json::Error> {
    serde_json::from_slice::<Unique>(bytes).map(|_| ())
}
#[test]
fn parity_rejects_missing_null_and_duplicate_keys() {
    // The original comparator's b[k] indexes a Map, not a Value: it already
    // panicked on an absent key. Keep that negative control when changing it.
    assert!(
        std::panic::catch_unwind(|| super::compare(
            "negative",
            &serde_json::json!({"a":null}),
            &serde_json::json!({"b":null})
        ))
        .is_err()
    );
    for bytes in [
        br#"{"a":1,"a":2}"#.as_slice(),
        br#"{"a":[{"b":null,"b":false}]}"#.as_slice(),
    ] {
        assert!(unique(bytes).is_err());
    }
    unique(br#"{"a":[{"b":null},true,1,1.2,"x"]}"#).unwrap();
}
