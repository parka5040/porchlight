use porchlight_core::ics::{self, EventOrigin, EventStatus, IcsTime, ImportedEvent, TimeMode};
use std::io::Write;

#[test]
fn audit_limits_incremental_ics_matches_legacy_wire_and_bounds_sink_writes() {
    let now = "2026-10-11T12:00:00Z".parse().unwrap();
    let event = ImportedEvent {
        uid: "fixture".into(),
        recurrence_id: None,
        origin: EventOrigin::Single,
        summary: "Meeting; café".into(),
        description: "Line one\r\nLine two, \\".repeat(1000),
        location: String::new(),
        start: IcsTime::Fixed(now),
        end: IcsTime::Fixed(now),
        all_day: false,
        time_mode: TimeMode::Fixed,
        tzid: None,
        rrule: None,
        exdates: Vec::new(),
        sequence: 1,
        status: EventStatus::Confirmed,
    };
    let expected = ics::write(std::slice::from_ref(&event), "Calendar", now);
    let mut bytes = Vec::new();
    let mut writer = ics::Writer::new(&mut bytes, "Calendar", now).unwrap();
    writer.event(&event, &Default::default()).unwrap();
    writer.finish().unwrap();
    assert_eq!(bytes, expected.as_bytes());
    struct CountingSink(usize);
    impl Write for CountingSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            assert!(
                bytes.len() <= 75,
                "incremental writer must not hand off a whole calendar"
            );
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = ics::Writer::new(CountingSink(0), "Calendar", now).unwrap();
    for _ in 0..1000 {
        writer.event(&event, &Default::default()).unwrap();
    }
    assert!(writer.finish().unwrap().0 > 20_000_000);
}
