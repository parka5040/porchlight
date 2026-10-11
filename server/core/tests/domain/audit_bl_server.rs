use porchlight_core::items::{
    Kind, OccurrenceAnchor, SeriesInput, SeriesSpec, TimeMode, persisted_occurrence_override,
};

#[test]
fn override_errors_name_override_fields() {
    for mode in [TimeMode::Fixed, TimeMode::Floating] {
        let mut input = SeriesInput {
            kind: Kind::Appointment,
            time_mode: mode,
            tzid: Some("America/New_York".into()),
            ..Default::default()
        };
        if mode == TimeMode::Fixed {
            input.starts_at = Some("2026-10-12T13:00:00Z".parse().unwrap());
            input.ends_at = Some("2026-10-12T14:00:00Z".parse().unwrap());
        } else {
            input.tzid = None;
            input.local_start = Some("2026-10-12T09:00:00".parse().unwrap());
            input.local_end = Some("2026-10-12T10:00:00".parse().unwrap());
        }
        let spec = SeriesSpec::new(input).unwrap();
        for (start, end, field) in [
            (
                "2026-10-12T10:00:00",
                "2026-10-12T09:00:00",
                "override_local_end",
            ),
            (
                "0000-10-12T09:00:00",
                "2026-10-12T10:00:00",
                "override_local_start",
            ),
        ] {
            let replacement = OccurrenceAnchor {
                seq: 0,
                starts_at: None,
                ends_at: None,
                due_at: None,
                due_date: None,
                local_due: None,
                local_start: Some(start.parse().unwrap()),
                local_end: Some(end.parse().unwrap()),
            };
            let err = persisted_occurrence_override(&spec, 0, &replacement).unwrap_err();
            assert_eq!(err.field, field, "{mode:?}");
        }
    }
}

#[test]
fn visibility_inventory_matches_derived_enum_variants() {
    use porchlight_core::access::Visibility;
    use serde::{
        Deserialize, Deserializer,
        de::{self, Visitor},
    };
    #[derive(Default)]
    struct Variants(Vec<&'static str>);
    impl<'de> Deserializer<'de> for &mut Variants {
        type Error = de::value::Error;
        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
            Err(de::Error::custom("only enum inventory is supported"))
        }
        fn deserialize_enum<V: Visitor<'de>>(
            self,
            _: &'static str,
            variants: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Self::Error> {
            self.0 = variants.to_vec();
            Err(de::Error::custom("inventory captured"))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf option
            unit unit_struct newtype_struct seq tuple tuple_struct map struct identifier ignored_any
        }
    }
    let mut variants = Variants::default();
    let _ = Visibility::deserialize(&mut variants);
    assert_eq!(
        Visibility::ALL.map(Visibility::as_str).as_slice(),
        variants.0
    );
}
