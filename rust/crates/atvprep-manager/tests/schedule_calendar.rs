//! Recorded local calendar and real-time DST boundaries, including repeated hours.
use atvprep_manager::schedules::{
    LocalDateTime, TimeOfDay, days_to_mask, latest_slot, local_to_utc, mask_to_days, next_run_at,
    next_slot,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
fn fields(value: Option<LocalDateTime>) -> Value {
    value.map_or(Value::Null, |value| {
        json!([
            value.year(),
            value.month(),
            value.day(),
            value.hour(),
            value.minute()
        ])
    })
}
fn utc(value: Option<DateTime<Utc>>) -> Value {
    value.map_or(Value::Null, |value| {
        json!(value.to_rfc3339_opts(SecondsFormat::Secs, true))
    })
}
#[test]
fn calendars_and_dst_match_the_python_oracle() {
    for case in atvprep_testkit::load_golden::<Value, Value>("schedules") {
        let input = case.input;
        let parts = input["local"].as_array().unwrap();
        let local = LocalDateTime::new(
            i32::try_from(parts[0].as_i64().unwrap()).unwrap(),
            u8::try_from(parts[1].as_u64().unwrap()).unwrap(),
            u8::try_from(parts[2].as_u64().unwrap()).unwrap(),
            u8::try_from(parts[3].as_u64().unwrap()).unwrap(),
            u8::try_from(parts[4].as_u64().unwrap()).unwrap(),
        )
        .unwrap();
        let mask = u8::try_from(input["mask"].as_u64().unwrap()).unwrap();
        let days: Vec<u8> = input["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| u8::try_from(value.as_u64().unwrap()).unwrap())
            .collect();
        let time = TimeOfDay::parse(input["time"].as_str().unwrap()).unwrap();
        let now = DateTime::parse_from_rfc3339(input["now"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        let observed = json!({"latest":fields(latest_slot(local,mask,time)),"next":fields(next_slot(local,mask,time)),"local_utc":utc(local_to_utc(local,&chrono_tz::Europe::Zurich)),"next_run":utc(next_run_at(now,input["enabled"].as_bool().unwrap(),mask,time,&chrono_tz::Europe::Zurich)),"days":mask_to_days(mask),"mask":days_to_mask(&days)});
        assert_eq!(observed, case.expected, "{}", case.name);
    }
}
