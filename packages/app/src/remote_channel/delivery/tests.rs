use super::*;

#[test]
fn solicited_delivery_bypasses_presence_and_other_delivery_preserves_it() {
    for (solicited, away, expected) in [
        (true, false, Delivery::Explicit),
        (true, true, Delivery::Explicit),
        (false, false, Delivery::Presence { away: false }),
        (false, true, Delivery::Presence { away: true }),
    ] {
        assert_eq!(remote_delivery(solicited, away), expected);
    }
}

#[test]
fn only_an_unsolicited_delivery_while_present_stops_at_a_channel_that_waits() {
    for (delivery, only_when_away, expected) in [
        (Delivery::Explicit, true, true),
        (Delivery::Explicit, false, true),
        (Delivery::Presence { away: true }, true, true),
        (Delivery::Presence { away: true }, false, true),
        (Delivery::Presence { away: false }, true, false),
        (Delivery::Presence { away: false }, false, true),
    ] {
        assert_eq!(
            delivery.reaches(only_when_away),
            expected,
            "{delivery:?} only_when_away={only_when_away}"
        );
    }
}
