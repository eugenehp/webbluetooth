//! Characteristic values, read as what they mean.
//!
//! A GATT value is bytes, and the generic renderings beside every value in this
//! program — hex, ASCII, a u16 taken little-endian — are the honest thing to
//! show when nobody knows what the bytes are. For the few hundred
//! characteristics the Bluetooth SIG has defined, somebody does know: the
//! encoding is in the profile specification, and showing `87 %` rather than
//! `u8 87` is the difference between reading a device and decoding one.
//!
//! Only characteristics whose encoding is stated plainly in their specification
//! are here, and each decoder refuses a value of the wrong length rather than
//! guessing. A wrong reading is worse than no reading: it looks like an answer.

use webbluetooth::uuid::BluetoothUuid;

/// One field read out of a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// What it is.
    pub name: &'static str,
    /// What it says, already formatted with its unit.
    pub value: String,
}

fn field(name: &'static str, value: impl Into<String>) -> Field {
    Field {
        name,
        value: value.into(),
    }
}

/// Read `bytes` as the characteristic `uuid` defines them.
///
/// Empty when nothing is known about this characteristic, or when the value is
/// not the shape its specification requires — a truncated packet is reported by
/// saying nothing rather than by inventing a reading from whatever arrived.
pub fn characteristic(uuid: &BluetoothUuid, bytes: &[u8]) -> Vec<Field> {
    // Vendor characteristics first: they are 128-bit, so they never collide
    // with an assigned number, and a device publishing one is usually the
    // reason you are looking at it.
    if let Some(fields) = vendor(uuid, bytes) {
        return fields;
    }
    let Some(short) = uuid.as_u16() else {
        return Vec::new();
    };
    match short {
        0x2A19 => battery_level(bytes),
        0x2A37 => heart_rate_measurement(bytes),
        0x2A38 => body_sensor_location(bytes),
        0x2A06 => alert_level(bytes),
        0x2A01 => appearance(bytes),
        0x2A04 => preferred_connection_parameters(bytes),
        0x2A50 => pnp_id(bytes),
        0x2A23 => system_id(bytes),
        0x2A6E => temperature(bytes),
        0x2A6F => humidity(bytes),
        0x2A6D => pressure(bytes),
        0x2A08 => date_time(bytes).map_or_else(Vec::new, |when| vec![field("date", when)]),
        0x2AA6 => central_address_resolution(bytes),
        0x2A1C => temperature_measurement(bytes),
        0x2A1D => temperature_type(bytes),
        0x2A07 => tx_power_level(bytes),
        0x2A05 => service_changed(bytes),
        // The time cluster.
        0x2A2B => current_time(bytes),
        0x2A09 => day_of_week_value(bytes),
        0x2A0A => day_date_time(bytes),
        0x2A0C => exact_time_256(bytes),
        0x2A0D => dst_offset(bytes),
        0x2A0E => time_zone(bytes),
        0x2A13 => time_source(bytes),
        // Blood pressure.
        0x2A35 | 0x2A36 => blood_pressure_measurement(bytes),
        // Weight and body.
        0x2A98 => weight(bytes),
        0x2A9D => weight_measurement(bytes),
        0x2A8E => height(bytes),
        0x2A80 => age(bytes),
        0x2A8C => gender(bytes),
        0x2A9A => user_index(bytes),
        // Cycling and running.
        0x2A5B => csc_measurement(bytes),
        0x2A53 => rsc_measurement(bytes),
        0x2A5D => sensor_location(bytes),
        // Glucose.
        0x2A18 => glucose_measurement(bytes),
        // Environmental sensing.
        0x2A6C => elevation(bytes),
        0x2A76 => uv_index(bytes),
        0x2A7B => dew_point(bytes),
        0x2A2C => magnetic_declination(bytes),
        0x2A77 => irradiance(bytes),
        // 0x2A70 True Wind Speed, 0x2A72 Apparent Wind Speed — both m/s in
        // hundredths. 0x2A71 and 0x2A73 are the matching *directions*, which
        // are degrees, not speeds.
        0x2A70 | 0x2A72 => wind_speed(bytes),
        0x2A71 | 0x2A73 => wind_direction(bytes),
        0x2A74 => gust_factor(bytes),
        0x2A75 => pollen_concentration(bytes),
        0x2A78 => rainfall(bytes),
        0x2A7A => heat_index(bytes),
        0x2AA3 => barometric_pressure_trend(bytes),
        // Reference and local time.
        0x2A0F => local_time_information(bytes),
        0x2A14 => reference_time_information(bytes),
        // Feature bitfields — what a device says it can do.
        0x2A49 => blood_pressure_feature(bytes),
        0x2A51 => glucose_feature(bytes),
        0x2A5C => csc_feature(bytes),
        0x2A54 => rsc_feature(bytes),
        0x2A42 | 0x2A47 => alert_category_bitmask(bytes),
        0x2A99 => database_change_increment(bytes),
        0x2A02 => peripheral_privacy_flag(bytes),
        0x2AC9 => resolvable_private_address_only(bytes),
        // Fitness Machine.
        0x2AD3 => training_status(bytes),
        0x2ACC => fitness_machine_feature(bytes),
        0x2AD4 | 0x2AD8 => supported_range_u16(bytes),
        0x2AD6 => supported_resistance_range(bytes),
        0x2AD7 => supported_heart_rate_range(bytes),
        // Continuous glucose monitoring.
        0x2AA7 => cgm_measurement(bytes),
        0x2AAB => cgm_session_run_time(bytes),
        0x2AAA => cgm_session_start_time(bytes),
        // The User Data service: a person's profile, as a fitness device
        // holds it.
        0x2A85 => date_of_birth(bytes),
        0x2A8A | 0x2A90 | 0x2A87 | 0x2AA2 => text(bytes),
        0x2A8D => beats_per_minute(bytes, "max heart rate"),
        0x2A92 => beats_per_minute(bytes, "resting heart rate"),
        0x2A7E => beats_per_minute(bytes, "aerobic threshold"),
        0x2A83 => beats_per_minute(bytes, "anaerobic threshold"),
        0x2A88 => beats_per_minute(bytes, "fat burn lower limit"),
        0x2A89 => beats_per_minute(bytes, "fat burn upper limit"),
        0x2A7F => beats_per_minute(bytes, "aerobic heart rate lower limit"),
        0x2A84 => beats_per_minute(bytes, "aerobic heart rate upper limit"),
        0x2A81 => beats_per_minute(bytes, "anaerobic heart rate lower limit"),
        0x2A82 => beats_per_minute(bytes, "anaerobic heart rate upper limit"),
        0x2A96 => vo2_max(bytes),
        0x2A8F => circumference(bytes, "hip circumference"),
        0x2A97 => circumference(bytes, "waist circumference"),
        0x2A93 => two_zone_limits(bytes),
        0x2A8B => five_zone_limits(bytes),
        // Scan Parameters.
        0x2A4F => scan_interval_window(bytes),
        0x2A31 => scan_refresh(bytes),
        // Alerts.
        0x2A3F => alert_status(bytes),
        0x2A41 => ringer_setting(bytes),
        0x2A43 => alert_category(bytes),
        0x2A45 => unread_alert_status(bytes),
        0x2A46 => new_alert(bytes),
        // The Device Information strings, and the Device Name.
        0x2A00 | 0x2A24 | 0x2A25 | 0x2A26 | 0x2A27 | 0x2A28 | 0x2A29 => text(bytes),
        _ => Vec::new(),
    }
}

/// Read `bytes` as the descriptor `uuid` defines them.
///
/// Descriptors are small and their encodings are short, which is exactly why
/// leaving them as hex is annoying: a Client Characteristic Configuration is
/// two bytes of which two bits matter, and reading `01 00` off the screen and
/// remembering which bit is which is a thing nobody should have to do twice.
pub fn descriptor(uuid: &BluetoothUuid, bytes: &[u8]) -> Vec<Field> {
    let Some(short) = uuid.as_u16() else {
        return Vec::new();
    };
    match short {
        0x2902 => client_characteristic_configuration(bytes),
        0x2900 => characteristic_extended_properties(bytes),
        0x2901 => text(bytes),
        0x2903 => server_characteristic_configuration(bytes),
        0x2904 => presentation_format(bytes),
        0x2906 => valid_range(bytes),
        _ => Vec::new(),
    }
}

fn client_characteristic_configuration(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let bits = u16::from_le_bytes([*low, *high]);
    let mut set = Vec::new();
    if bits & 0x0001 != 0 {
        set.push("notifications");
    }
    if bits & 0x0002 != 0 {
        set.push("indications");
    }
    vec![field(
        "subscribed to",
        if set.is_empty() {
            "nothing".to_owned()
        } else {
            set.join(" and ")
        },
    )]
}

fn server_characteristic_configuration(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let bits = u16::from_le_bytes([*low, *high]);
    vec![field(
        "broadcasts",
        if bits & 0x0001 != 0 { "yes" } else { "no" },
    )]
}

fn characteristic_extended_properties(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let bits = u16::from_le_bytes([*low, *high]);
    let mut set = Vec::new();
    if bits & 0x0001 != 0 {
        set.push("reliable write");
    }
    if bits & 0x0002 != 0 {
        set.push("writable auxiliaries");
    }
    vec![field(
        "extended",
        if set.is_empty() {
            "none".to_owned()
        } else {
            set.join(", ")
        },
    )]
}

/// Characteristic Presentation Format: how to read the characteristic it is on.
///
/// The descriptor that makes a vendor characteristic self-describing, and
/// almost nothing reads it.
fn presentation_format(bytes: &[u8]) -> Vec<Field> {
    let Some([format, exponent, u0, u1, namespace, d0, d1]) = bytes.get(..7) else {
        return Vec::new();
    };
    let unit = u16::from_le_bytes([*u0, *u1]);
    let description = u16::from_le_bytes([*d0, *d1]);
    vec![
        field("format", format_name(*format)),
        field("exponent", (*exponent as i8).to_string()),
        field("unit", unit_name(unit)),
        field(
            "namespace",
            if *namespace == 1 {
                "Bluetooth SIG".to_owned()
            } else {
                format!("0x{namespace:02X}")
            },
        ),
        field("description", format!("0x{description:04X}")),
    ]
}

fn format_name(code: u8) -> String {
    let name = match code {
        0x01 => "boolean",
        0x02 => "2-bit",
        0x03 => "nibble",
        0x04 => "uint8",
        0x05 => "uint12",
        0x06 => "uint16",
        0x07 => "uint24",
        0x08 => "uint32",
        0x09 => "uint48",
        0x0A => "uint64",
        0x0B => "uint128",
        0x0C => "sint8",
        0x0D => "sint12",
        0x0E => "sint16",
        0x0F => "sint24",
        0x10 => "sint32",
        0x11 => "sint48",
        0x12 => "sint64",
        0x13 => "sint128",
        0x14 => "float32",
        0x15 => "float64",
        0x16 => "SFLOAT",
        0x17 => "FLOAT",
        0x18 => "IEEE-20601",
        0x19 => "UTF-8",
        0x1A => "UTF-16",
        0x1B => "opaque",
        _ => return format!("0x{code:02X}"),
    };
    name.to_owned()
}

/// The handful of units common enough to be worth naming.
fn unit_name(code: u16) -> String {
    let name = match code {
        0x2700 => "unitless",
        0x2703 => "seconds",
        0x2713 => "m/s²",
        0x2724 => "pascal",
        0x2728 => "volt",
        0x2729 => "ampere",
        0x272F => "°C",
        0x2731 => "watt",
        0x2757 => "hertz",
        0x27AD => "percent",
        0x27B2 => "dB",
        0x2A70 => "km/h",
        _ => return format!("0x{code:04X}"),
    };
    name.to_owned()
}

/// Valid Range: two values in the characteristic's own format.
///
/// The format is only knowable from the Presentation Format descriptor beside
/// it, so the bytes are reported split rather than interpreted.
fn valid_range(bytes: &[u8]) -> Vec<Field> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return Vec::new();
    }
    let (lower, upper) = bytes.split_at(bytes.len() / 2);
    vec![
        field("lower", crate::format::hex(lower)),
        field("upper", crate::format::hex(upper)),
    ]
}

/// A `uint16` scaled by a power of ten, with a unit.
fn scaled_u16(bytes: &[u8], scale: f32, unit: &str, name: &'static str) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let raw = u16::from_le_bytes([*low, *high]);
    vec![field(name, format!("{:.2} {unit}", f32::from(raw) * scale))]
}

/// A `sint16` scaled by a power of ten, with a unit.
fn scaled_i16(bytes: &[u8], scale: f32, unit: &str, name: &'static str) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let raw = i16::from_le_bytes([*low, *high]);
    vec![field(name, format!("{:.2} {unit}", f32::from(raw) * scale))]
}

fn tx_power_level(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [level] => vec![field("tx power", format!("{} dBm", *level as i8))],
        _ => Vec::new(),
    }
}

fn temperature_type(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [1] => "Armpit",
        [2] => "Body (general)",
        [3] => "Ear (usually earlobe)",
        [4] => "Finger",
        [5] => "Gastrointestinal tract",
        [6] => "Mouth",
        [7] => "Rectum",
        [8] => "Toe",
        [9] => "Tympanum (ear drum)",
        _ => return Vec::new(),
    };
    vec![field("site", name)]
}

/// Service Changed: the handle range whose definitions moved.
fn service_changed(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c, d] = bytes else {
        return Vec::new();
    };
    vec![field(
        "handles changed",
        format!(
            "0x{:04X}–0x{:04X}",
            u16::from_le_bytes([*a, *b]),
            u16::from_le_bytes([*c, *d])
        ),
    )]
}

fn day_of_week_name(day: u8) -> &'static str {
    match day {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        7 => "Sunday",
        _ => "unknown",
    }
}

fn day_of_week_value(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [day] => vec![field("day", day_of_week_name(*day))],
        _ => Vec::new(),
    }
}

/// Day Date Time: a Date Time, then the day of the week.
fn day_date_time(bytes: &[u8]) -> Vec<Field> {
    let Some(when) = date_time(bytes) else {
        return Vec::new();
    };
    let Some(day) = bytes.get(7) else {
        return Vec::new();
    };
    vec![field("date", when), field("day", day_of_week_name(*day))]
}

/// Exact Time 256: a Day Date Time, then 1/256ths of a second.
fn exact_time_256(bytes: &[u8]) -> Vec<Field> {
    let mut out = day_date_time(bytes);
    if out.is_empty() {
        return out;
    }
    if let Some(fractions) = bytes.get(8) {
        out.push(field(
            "fraction",
            format!("{:.3} s", f32::from(*fractions) / 256.0),
        ));
    }
    out
}

/// Current Time: an Exact Time 256, then why it was last adjusted.
fn current_time(bytes: &[u8]) -> Vec<Field> {
    let mut out = exact_time_256(bytes);
    if out.is_empty() {
        return out;
    }
    let Some(reason) = bytes.get(9) else {
        return out;
    };
    let mut set = Vec::new();
    if reason & 0x01 != 0 {
        set.push("manual update");
    }
    if reason & 0x02 != 0 {
        set.push("external reference");
    }
    if reason & 0x04 != 0 {
        set.push("time zone change");
    }
    if reason & 0x08 != 0 {
        set.push("DST change");
    }
    out.push(field(
        "adjust reason",
        if set.is_empty() {
            "none".to_owned()
        } else {
            set.join(", ")
        },
    ));
    out
}

fn dst_offset(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "standard time",
        [2] => "+0.5 h (half an hour daylight time)",
        [4] => "+1 h (daylight time)",
        [8] => "+2 h (double daylight time)",
        [255] => "unknown",
        _ => return Vec::new(),
    };
    vec![field("DST offset", name)]
}

/// Time Zone: a signed count of quarter-hours from UTC.
fn time_zone(bytes: &[u8]) -> Vec<Field> {
    let [raw] = bytes else {
        return Vec::new();
    };
    let quarters = *raw as i8;
    if quarters == -128 {
        return vec![field("time zone", "unknown")];
    }
    let minutes = i32::from(quarters) * 15;
    vec![field(
        "time zone",
        format!(
            "UTC{}{:02}:{:02}",
            if minutes < 0 { "-" } else { "+" },
            minutes.abs() / 60,
            minutes.abs() % 60
        ),
    )]
}

fn time_source(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "unknown",
        [1] => "Network Time Protocol",
        [2] => "GPS",
        [3] => "radio time signal",
        [4] => "manual",
        [5] => "atomic clock",
        [6] => "cellular network",
        _ => return Vec::new(),
    };
    vec![field("time source", name)]
}

/// Blood Pressure Measurement, and Intermediate Cuff Pressure, which share a
/// layout: flags, three SFLOATs, then optional fields.
fn blood_pressure_measurement(bytes: &[u8]) -> Vec<Field> {
    let [flags, rest @ ..] = bytes else {
        return Vec::new();
    };
    let unit = if flags & 0x01 != 0 { "kPa" } else { "mmHg" };
    let Some(systolic) = rest.get(..2).and_then(sfloat) else {
        return Vec::new();
    };
    let Some(diastolic) = rest.get(2..4).and_then(sfloat) else {
        return Vec::new();
    };
    let Some(mean) = rest.get(4..6).and_then(sfloat) else {
        return Vec::new();
    };
    let mut out = vec![
        field("systolic", format!("{systolic:.1} {unit}")),
        field("diastolic", format!("{diastolic:.1} {unit}")),
        field("mean arterial", format!("{mean:.1} {unit}")),
    ];

    let mut at = 6;
    if flags & 0x02 != 0 {
        if let Some(when) = rest.get(at..at + 7).and_then(date_time) {
            out.push(field("taken", when));
        }
        at += 7;
    }
    if flags & 0x04 != 0 {
        if let Some(rate) = rest.get(at..at + 2).and_then(sfloat) {
            out.push(field("pulse", format!("{rate:.0} bpm")));
        }
    }
    out
}

/// IEEE-11073 16-bit SFLOAT: a 4-bit exponent and a 12-bit signed mantissa.
fn sfloat(bytes: &[u8]) -> Option<f64> {
    let [low, high] = bytes else {
        return None;
    };
    let raw = u16::from_le_bytes([*low, *high]);
    let exponent_raw = (raw >> 12) as u8;
    let exponent = if exponent_raw & 0x08 != 0 {
        i8::try_from(exponent_raw).ok()? - 16
    } else {
        i8::try_from(exponent_raw).ok()?
    };
    let mantissa_raw = raw & 0x0FFF;
    let mantissa = if mantissa_raw & 0x0800 != 0 {
        i32::from(mantissa_raw) - 4096
    } else {
        i32::from(mantissa_raw)
    };
    // The reserved values are statements about the reading, not numbers.
    match mantissa {
        0x07FF | -0x0800 | 0x0800 | 0x0801 | 0x0802 => None,
        _ => Some(f64::from(mantissa) * 10f64.powi(i32::from(exponent))),
    }
}

fn weight(bytes: &[u8]) -> Vec<Field> {
    scaled_u16(bytes, 0.005, "kg", "weight")
}

fn height(bytes: &[u8]) -> Vec<Field> {
    scaled_u16(bytes, 0.01, "m", "height")
}

fn age(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [years] => vec![field("age", format!("{years} years"))],
        _ => Vec::new(),
    }
}

fn gender(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "Male",
        [1] => "Female",
        [2] => "Unspecified",
        _ => return Vec::new(),
    };
    vec![field("gender", name)]
}

fn user_index(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [0xFF] => vec![field("user", "unknown")],
        [index] => vec![field("user", index.to_string())],
        _ => Vec::new(),
    }
}

/// Weight Measurement: flags decide the units and what follows.
fn weight_measurement(bytes: &[u8]) -> Vec<Field> {
    let [flags, rest @ ..] = bytes else {
        return Vec::new();
    };
    let imperial = flags & 0x01 != 0;
    let [low, high, rest @ ..] = rest else {
        return Vec::new();
    };
    let raw = u16::from_le_bytes([*low, *high]);
    // Half-pounds imperial, five-gram steps metric.
    let (value, unit) = if imperial {
        (f32::from(raw) * 0.01, "lb")
    } else {
        (f32::from(raw) * 0.005, "kg")
    };
    let mut out = vec![field("weight", format!("{value:.2} {unit}"))];

    let mut at = 0;
    if flags & 0x02 != 0 {
        if let Some(when) = rest.get(at..at + 7).and_then(date_time) {
            out.push(field("taken", when));
        }
        at += 7;
    }
    if flags & 0x04 != 0 {
        if let Some(index) = rest.get(at) {
            out.push(field("user", index.to_string()));
        }
    }
    out
}

fn sensor_location(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "Other",
        [1] => "Top of shoe",
        [2] => "In shoe",
        [3] => "Hip",
        [4] => "Front wheel",
        [5] => "Left crank",
        [6] => "Right crank",
        [7] => "Left pedal",
        [8] => "Right pedal",
        [9] => "Front hub",
        [10] => "Rear dropout",
        [11] => "Chainstay",
        [12] => "Rear wheel",
        [13] => "Rear hub",
        [14] => "Chest",
        [15] => "Spider",
        [16] => "Chain ring",
        _ => return Vec::new(),
    };
    vec![field("location", name)]
}

/// Cycling Speed and Cadence Measurement.
fn csc_measurement(bytes: &[u8]) -> Vec<Field> {
    let [flags, rest @ ..] = bytes else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut at = 0;

    if flags & 0x01 != 0 {
        let Some(revolutions) = rest.get(at..at + 4) else {
            return out;
        };
        out.push(field(
            "wheel revolutions",
            u32::from_le_bytes([
                revolutions[0],
                revolutions[1],
                revolutions[2],
                revolutions[3],
            ])
            .to_string(),
        ));
        at += 4;
        if let Some(event) = rest.get(at..at + 2) {
            // 1/1024 s units.
            out.push(field(
                "last wheel event",
                format!(
                    "{:.3} s",
                    f32::from(u16::from_le_bytes([event[0], event[1]])) / 1024.0
                ),
            ));
        }
        at += 2;
    }
    if flags & 0x02 != 0 {
        if let Some(crank) = rest.get(at..at + 2) {
            out.push(field(
                "crank revolutions",
                u16::from_le_bytes([crank[0], crank[1]]).to_string(),
            ));
        }
        at += 2;
        if let Some(event) = rest.get(at..at + 2) {
            out.push(field(
                "last crank event",
                format!(
                    "{:.3} s",
                    f32::from(u16::from_le_bytes([event[0], event[1]])) / 1024.0
                ),
            ));
        }
    }
    out
}

/// Running Speed and Cadence Measurement.
fn rsc_measurement(bytes: &[u8]) -> Vec<Field> {
    let [flags, s0, s1, cadence, rest @ ..] = bytes else {
        return Vec::new();
    };
    // Speed is in 1/256 m/s.
    let speed = f32::from(u16::from_le_bytes([*s0, *s1])) / 256.0;
    let mut out = vec![
        field("speed", format!("{speed:.2} m/s")),
        field("cadence", format!("{cadence} steps/min")),
        field(
            "moving",
            if flags & 0x04 != 0 {
                "running"
            } else {
                "walking"
            },
        ),
    ];

    let mut at = 0;
    if flags & 0x01 != 0 {
        if let Some(length) = rest.get(at..at + 2) {
            out.push(field(
                "stride length",
                format!("{} cm", u16::from_le_bytes([length[0], length[1]])),
            ));
        }
        at += 2;
    }
    if flags & 0x02 != 0 {
        if let Some(distance) = rest.get(at..at + 4) {
            // Decimetres.
            let raw = u32::from_le_bytes([distance[0], distance[1], distance[2], distance[3]]);
            out.push(field(
                "total distance",
                format!("{:.1} m", raw as f32 / 10.0),
            ));
        }
    }
    out
}

/// Glucose Measurement: flags, sequence, time, then optionals.
fn glucose_measurement(bytes: &[u8]) -> Vec<Field> {
    let [flags, s0, s1, rest @ ..] = bytes else {
        return Vec::new();
    };
    let mut out = vec![field(
        "sequence",
        u16::from_le_bytes([*s0, *s1]).to_string(),
    )];
    let Some(when) = rest.get(..7).and_then(date_time) else {
        return out;
    };
    out.push(field("taken", when));

    let mut at = 7;
    if flags & 0x01 != 0 {
        if let Some(offset) = rest.get(at..at + 2) {
            out.push(field(
                "time offset",
                format!("{} min", i16::from_le_bytes([offset[0], offset[1]])),
            ));
        }
        at += 2;
    }
    if flags & 0x02 != 0 {
        if let Some(concentration) = rest.get(at..at + 2).and_then(sfloat) {
            // Bit 2 chooses the unit.
            let (scale, unit) = if flags & 0x04 != 0 {
                (1000.0, "mol/L")
            } else {
                (100_000.0, "kg/L")
            };
            out.push(field(
                "glucose",
                format!("{:.1} m{unit}", concentration * scale),
            ));
        }
    }
    out
}

fn elevation(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c] = bytes else {
        return Vec::new();
    };
    // A 24-bit signed value in centimetres.
    let raw = i32::from_le_bytes([*a, *b, *c, if *c & 0x80 != 0 { 0xFF } else { 0x00 }]);
    vec![field("elevation", format!("{:.2} m", raw as f32 / 100.0))]
}

fn uv_index(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [index] => vec![field("UV index", index.to_string())],
        _ => Vec::new(),
    }
}

fn dew_point(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [value] => vec![field("dew point", format!("{} °C", *value as i8))],
        _ => Vec::new(),
    }
}

fn magnetic_declination(bytes: &[u8]) -> Vec<Field> {
    scaled_u16(bytes, 0.01, "°", "declination")
}

fn irradiance(bytes: &[u8]) -> Vec<Field> {
    scaled_u16(bytes, 0.1, "W/m²", "irradiance")
}

fn wind_speed(bytes: &[u8]) -> Vec<Field> {
    scaled_u16(bytes, 0.01, "m/s", "wind speed")
}

fn wind_direction(bytes: &[u8]) -> Vec<Field> {
    scaled_u16(bytes, 0.01, "°", "wind direction")
}

fn gust_factor(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [raw] => vec![field(
            "gust factor",
            format!("{:.1}", f32::from(*raw) * 0.1),
        )],
        _ => Vec::new(),
    }
}

fn pollen_concentration(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c] = bytes else {
        return Vec::new();
    };
    // uint24, in grains per cubic metre.
    let raw = u32::from_le_bytes([*a, *b, *c, 0]);
    vec![field("pollen", format!("{raw} grains/m³"))]
}

fn rainfall(bytes: &[u8]) -> Vec<Field> {
    scaled_u16(bytes, 0.001, "m", "rainfall")
}

fn heat_index(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [raw] => vec![field("heat index", format!("{} °C", *raw as i8))],
        _ => Vec::new(),
    }
}

fn barometric_pressure_trend(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "Unknown",
        [1] => "Continuously falling",
        [2] => "Continuously rising",
        [3] => "Falling, then steady",
        [4] => "Rising, then steady",
        [5] => "Falling before a lesser rise",
        [6] => "Falling before a greater rise",
        [7] => "Rising before a greater fall",
        [8] => "Rising before a lesser fall",
        [9] => "Steady",
        _ => return Vec::new(),
    };
    vec![field("trend", name)]
}

/// Local Time Information: a time zone and a DST offset.
fn local_time_information(bytes: &[u8]) -> Vec<Field> {
    let [zone, dst] = bytes else {
        return Vec::new();
    };
    let mut out = time_zone(&[*zone]);
    out.extend(dst_offset(&[*dst]));
    out
}

/// Reference Time Information: where the time came from and how stale it is.
fn reference_time_information(bytes: &[u8]) -> Vec<Field> {
    let [source, accuracy, days, hours] = bytes else {
        return Vec::new();
    };
    let mut out = time_source(&[*source]);
    out.push(field(
        "accuracy",
        match accuracy {
            // 254 is "accuracy out of range", 255 "unknown"; everything else
            // is an eighth of a second per unit.
            254 => "out of range".to_owned(),
            255 => "unknown".to_owned(),
            other => format!("±{:.3} s", f32::from(*other) * 0.125),
        },
    ));
    out.push(field(
        "since update",
        if *days == 255 && *hours == 255 {
            "255 days or more".to_owned()
        } else {
            format!("{days} d {hours} h")
        },
    ));
    out
}

/// Render a bitfield as the names of the bits that are set.
fn bits(value: u32, names: &[(u32, &str)]) -> String {
    let set: Vec<&str> = names
        .iter()
        .filter(|(bit, _)| value & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    if set.is_empty() {
        "none".to_owned()
    } else {
        set.join(", ")
    }
}

fn blood_pressure_feature(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let value = u32::from(u16::from_le_bytes([*low, *high]));
    vec![field(
        "supports",
        bits(
            value,
            &[
                (0x01, "body movement detection"),
                (0x02, "cuff fit detection"),
                (0x04, "irregular pulse detection"),
                (0x08, "pulse rate range detection"),
                (0x10, "measurement position detection"),
                (0x20, "multiple bonds"),
            ],
        ),
    )]
}

fn glucose_feature(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let value = u32::from(u16::from_le_bytes([*low, *high]));
    vec![field(
        "supports",
        bits(
            value,
            &[
                (0x001, "low battery detection"),
                (0x002, "sensor malfunction detection"),
                (0x004, "sensor sample size"),
                (0x008, "strip insertion error detection"),
                (0x010, "strip type error detection"),
                (0x020, "result high-low detection"),
                (0x040, "temperature high-low detection"),
                (0x080, "read interrupt detection"),
                (0x100, "general device fault"),
                (0x200, "time fault"),
                (0x400, "multiple bonds"),
            ],
        ),
    )]
}

fn csc_feature(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let value = u32::from(u16::from_le_bytes([*low, *high]));
    vec![field(
        "supports",
        bits(
            value,
            &[
                (0x01, "wheel revolutions"),
                (0x02, "crank revolutions"),
                (0x04, "multiple sensor locations"),
            ],
        ),
    )]
}

fn rsc_feature(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let value = u32::from(u16::from_le_bytes([*low, *high]));
    vec![field(
        "supports",
        bits(
            value,
            &[
                (0x01, "stride length"),
                (0x02, "total distance"),
                (0x04, "walking or running status"),
                (0x08, "calibration"),
                (0x10, "multiple sensor locations"),
            ],
        ),
    )]
}

/// A bitmask over the ten alert categories.
fn alert_category_bitmask(bytes: &[u8]) -> Vec<Field> {
    if bytes.is_empty() || bytes.len() > 2 {
        return Vec::new();
    }
    let mut value = u32::from(bytes[0]);
    if let Some(high) = bytes.get(1) {
        value |= u32::from(*high) << 8;
    }
    let set: Vec<&str> = (0..10u8)
        .filter(|bit| value & (1 << bit) != 0)
        .map(alert_category_name)
        .collect();
    vec![field(
        "categories",
        if set.is_empty() {
            "none".to_owned()
        } else {
            set.join(", ")
        },
    )]
}

fn database_change_increment(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c, d] = bytes else {
        return Vec::new();
    };
    vec![field(
        "increment",
        u32::from_le_bytes([*a, *b, *c, *d]).to_string(),
    )]
}

fn peripheral_privacy_flag(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "privacy disabled",
        [1] => "privacy enabled",
        _ => return Vec::new(),
    };
    vec![field("privacy", name)]
}

fn resolvable_private_address_only(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "may use other address types",
        [1] => "only resolvable private addresses",
        _ => return Vec::new(),
    };
    vec![field("addressing", name)]
}

fn training_status(bytes: &[u8]) -> Vec<Field> {
    let [flags, status, rest @ ..] = bytes else {
        return Vec::new();
    };
    let name = match status {
        0x00 => "Other",
        0x01 => "Idle",
        0x02 => "Warming Up",
        0x03 => "Low Intensity Interval",
        0x04 => "High Intensity Interval",
        0x05 => "Recovery Interval",
        0x06 => "Isometric",
        0x07 => "Heart Rate Control",
        0x08 => "Fitness Test",
        0x09 => "Speed Outside of Control Region — Low",
        0x0A => "Speed Outside of Control Region — High",
        0x0B => "Cool Down",
        0x0C => "Watt Control",
        0x0D => "Manual Mode",
        0x0E => "Pre-Workout",
        0x0F => "Post-Workout",
        _ => "unknown",
    };
    let mut out = vec![field("status", name)];
    // Bit 0 says a free-text string follows.
    if flags & 0x01 != 0 {
        if let Ok(text) = std::str::from_utf8(rest) {
            if !text.is_empty() {
                out.push(field("detail", text));
            }
        }
    }
    out
}

fn fitness_machine_feature(bytes: &[u8]) -> Vec<Field> {
    let Some(machine) = bytes.get(..4) else {
        return Vec::new();
    };
    let value = u32::from_le_bytes([machine[0], machine[1], machine[2], machine[3]]);
    vec![field(
        "reports",
        bits(
            value,
            &[
                (0x0000_0001, "average speed"),
                (0x0000_0002, "cadence"),
                (0x0000_0004, "total distance"),
                (0x0000_0008, "inclination"),
                (0x0000_0010, "elevation gain"),
                (0x0000_0020, "pace"),
                (0x0000_0040, "step count"),
                (0x0000_0080, "resistance level"),
                (0x0000_0100, "stride count"),
                (0x0000_0200, "expended energy"),
                (0x0000_0400, "heart rate"),
                (0x0000_0800, "metabolic equivalent"),
                (0x0000_1000, "elapsed time"),
                (0x0000_2000, "remaining time"),
                (0x0000_4000, "power measurement"),
                (0x0000_8000, "force on belt and power output"),
                (0x0001_0000, "user data retention"),
            ],
        ),
    )]
}

/// Supported Speed Range and Supported Power Range: minimum, maximum, step.
fn supported_range_u16(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c, d, e, f] = bytes else {
        return Vec::new();
    };
    vec![
        field("minimum", u16::from_le_bytes([*a, *b]).to_string()),
        field("maximum", u16::from_le_bytes([*c, *d]).to_string()),
        field("step", u16::from_le_bytes([*e, *f]).to_string()),
    ]
}

/// Supported Resistance Level Range: the same, signed.
fn supported_resistance_range(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c, d, e, f] = bytes else {
        return Vec::new();
    };
    vec![
        field(
            "minimum",
            format!("{:.1}", f32::from(i16::from_le_bytes([*a, *b])) * 0.1),
        ),
        field(
            "maximum",
            format!("{:.1}", f32::from(i16::from_le_bytes([*c, *d])) * 0.1),
        ),
        field(
            "step",
            format!("{:.1}", f32::from(u16::from_le_bytes([*e, *f])) * 0.1),
        ),
    ]
}

/// Supported Heart Rate Range: three single bytes, in beats per minute.
fn supported_heart_rate_range(bytes: &[u8]) -> Vec<Field> {
    let [min, max, step] = bytes else {
        return Vec::new();
    };
    vec![
        field("minimum", format!("{min} bpm")),
        field("maximum", format!("{max} bpm")),
        field("step", format!("{step} bpm")),
    ]
}

/// CGM Measurement: a length byte, flags, a concentration and a time offset.
///
/// The leading size byte is what makes this different from every other
/// measurement characteristic — several records can be packed into one value.
fn cgm_measurement(bytes: &[u8]) -> Vec<Field> {
    let [size, flags, rest @ ..] = bytes else {
        return Vec::new();
    };
    let Some(concentration) = rest.get(..2).and_then(sfloat) else {
        return Vec::new();
    };
    let Some(offset) = rest.get(2..4) else {
        return Vec::new();
    };
    let mut out = vec![
        field("glucose", format!("{concentration:.1} mg/dL")),
        field(
            "time offset",
            format!("{} min", u16::from_le_bytes([offset[0], offset[1]])),
        ),
    ];
    if usize::from(*size) != bytes.len() {
        // Several records packed together, or a truncated one. Either way the
        // reading above is the first record and the rest is not decoded.
        out.push(field(
            "record",
            format!("{size} of {} bytes — more records follow", bytes.len()),
        ));
    }
    let _ = flags;
    out
}

fn cgm_session_run_time(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    vec![field(
        "session run time",
        format!("{} h", u16::from_le_bytes([*low, *high])),
    )]
}

/// CGM Session Start Time: a Date Time, a time zone and a DST offset.
fn cgm_session_start_time(bytes: &[u8]) -> Vec<Field> {
    let Some(when) = date_time(bytes) else {
        return Vec::new();
    };
    let mut out = vec![field("started", when)];
    if let Some(zone) = bytes.get(7) {
        out.extend(time_zone(&[*zone]));
    }
    if let Some(dst) = bytes.get(8) {
        out.extend(dst_offset(&[*dst]));
    }
    out
}

/// Date of Birth: a year, a month and a day, with zero meaning "not known".
fn date_of_birth(bytes: &[u8]) -> Vec<Field> {
    let [y0, y1, month, day] = bytes else {
        return Vec::new();
    };
    let year = u16::from_le_bytes([*y0, *y1]);
    vec![field("born", format!("{year:04}-{month:02}-{day:02}"))]
}

fn beats_per_minute(bytes: &[u8], name: &'static str) -> Vec<Field> {
    match bytes {
        [value] => vec![field(name, format!("{value} bpm"))],
        _ => Vec::new(),
    }
}

fn vo2_max(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [value] => vec![field("VO₂ max", format!("{value} ml/kg/min"))],
        _ => Vec::new(),
    }
}

fn circumference(bytes: &[u8], name: &'static str) -> Vec<Field> {
    scaled_u16(bytes, 0.01, "m", name)
}

/// Two Zone Heart Rate Limits: the boundary between fat burn and fitness.
fn two_zone_limits(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [limit] => vec![field("fat burn / fitness boundary", format!("{limit} bpm"))],
        _ => Vec::new(),
    }
}

/// Five Zone Heart Rate Limits: four boundaries between five zones.
fn five_zone_limits(bytes: &[u8]) -> Vec<Field> {
    let [very_light, light, moderate, hard] = bytes else {
        return Vec::new();
    };
    vec![
        field("very light / light", format!("{very_light} bpm")),
        field("light / moderate", format!("{light} bpm")),
        field("moderate / hard", format!("{moderate} bpm")),
        field("hard / maximum", format!("{hard} bpm")),
    ]
}

/// Scan Interval Window: how often, and for how long, a central scans.
fn scan_interval_window(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c, d] = bytes else {
        return Vec::new();
    };
    let interval = u16::from_le_bytes([*a, *b]);
    let window = u16::from_le_bytes([*c, *d]);
    // Both in 0.625 ms units.
    vec![
        field(
            "interval",
            format!("{:.2} ms ({interval})", f32::from(interval) * 0.625),
        ),
        field(
            "window",
            format!("{:.2} ms ({window})", f32::from(window) * 0.625),
        ),
    ]
}

fn scan_refresh(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [0] => vec![field("scan refresh", "server requires refresh")],
        [other] => vec![field("scan refresh", format!("0x{other:02X}"))],
        _ => Vec::new(),
    }
}

fn alert_status(bytes: &[u8]) -> Vec<Field> {
    let [bits] = bytes else {
        return Vec::new();
    };
    let mut set = Vec::new();
    if bits & 0x01 != 0 {
        set.push("ringer active");
    }
    if bits & 0x02 != 0 {
        set.push("vibrate active");
    }
    if bits & 0x04 != 0 {
        set.push("display alert active");
    }
    vec![field(
        "alerting",
        if set.is_empty() {
            "nothing".to_owned()
        } else {
            set.join(", ")
        },
    )]
}

fn ringer_setting(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "Ringer silent",
        [1] => "Ringer normal",
        _ => return Vec::new(),
    };
    vec![field("ringer", name)]
}

fn alert_category_name(id: u8) -> &'static str {
    match id {
        0 => "Simple Alert",
        1 => "Email",
        2 => "News",
        3 => "Call",
        4 => "Missed Call",
        5 => "SMS or MMS",
        6 => "Voice Mail",
        7 => "Schedule",
        8 => "High Prioritised Alert",
        9 => "Instant Message",
        _ => "unknown",
    }
}

fn alert_category(bytes: &[u8]) -> Vec<Field> {
    match bytes {
        [id] => vec![field("category", alert_category_name(*id))],
        _ => Vec::new(),
    }
}

fn unread_alert_status(bytes: &[u8]) -> Vec<Field> {
    let [id, count] = bytes else {
        return Vec::new();
    };
    vec![
        field("category", alert_category_name(*id)),
        field("unread", count.to_string()),
    ]
}

/// New Alert: a category, a count, and optionally the latest alert as text.
fn new_alert(bytes: &[u8]) -> Vec<Field> {
    let [id, count, rest @ ..] = bytes else {
        return Vec::new();
    };
    let mut out = vec![
        field("category", alert_category_name(*id)),
        field("new", count.to_string()),
    ];
    if let Ok(text) = std::str::from_utf8(rest) {
        if !text.is_empty() {
            out.push(field("text", text));
        }
    }
    out
}

/// Characteristics defined by somebody other than the Bluetooth SIG.
///
/// `None` means "not one of these", which is different from "one of these and
/// unreadable" — the caller falls through to the assigned numbers.
fn vendor(uuid: &BluetoothUuid, bytes: &[u8]) -> Option<Vec<Field>> {
    match uuid.as_u128() {
        // ANCS Notification Source, `9FBF120D-6301-42D9-8C58-25E699A21DBD`.
        // Apple publishes this format; it is what an iPhone tells a watch.
        0x9FBF_120D_6301_42D9_8C58_25E6_99A2_1DBD => Some(ancs_notification_source(bytes)),
        _ => None,
    }
}

/// ANCS Notification Source: event, flags, category, count, and an id.
fn ancs_notification_source(bytes: &[u8]) -> Vec<Field> {
    let [event, flags, category, count, a, b, c, d] = bytes else {
        return Vec::new();
    };
    let event_name = match event {
        0 => "Added",
        1 => "Modified",
        2 => "Removed",
        _ => "unknown",
    };
    let category_name = match category {
        0 => "Other",
        1 => "Incoming Call",
        2 => "Missed Call",
        3 => "Voicemail",
        4 => "Social",
        5 => "Schedule",
        6 => "Email",
        7 => "News",
        8 => "Health and Fitness",
        9 => "Business and Finance",
        10 => "Location",
        11 => "Entertainment",
        _ => "unknown",
    };
    vec![
        field("event", event_name),
        field(
            "flags",
            bits(
                u32::from(*flags),
                &[
                    (0x01, "silent"),
                    (0x02, "important"),
                    (0x04, "pre-existing"),
                    (0x08, "positive action"),
                    (0x10, "negative action"),
                ],
            ),
        ),
        field("category", category_name),
        field("in category", count.to_string()),
        field(
            "notification id",
            u32::from_le_bytes([*a, *b, *c, *d]).to_string(),
        ),
    ]
}

fn text(bytes: &[u8]) -> Vec<Field> {
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.is_empty() => vec![field("text", text)],
        _ => Vec::new(),
    }
}

fn battery_level(bytes: &[u8]) -> Vec<Field> {
    // "The value 0x00 represents 0% and 0x64 represents 100%." Anything above
    // is out of range, and saying so beats printing 200%.
    match bytes {
        [level @ 0..=100] => vec![field("battery", format!("{level} %"))],
        [level] => vec![field("battery", format!("{level} — out of range, 0..100"))],
        _ => Vec::new(),
    }
}

fn alert_level(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "No Alert",
        [1] => "Mild Alert",
        [2] => "High Alert",
        _ => return Vec::new(),
    };
    vec![field("alert", name)]
}

fn body_sensor_location(bytes: &[u8]) -> Vec<Field> {
    let name = match bytes {
        [0] => "Other",
        [1] => "Chest",
        [2] => "Wrist",
        [3] => "Finger",
        [4] => "Hand",
        [5] => "Ear Lobe",
        [6] => "Foot",
        _ => return Vec::new(),
    };
    vec![field("location", name)]
}

fn central_address_resolution(bytes: &[u8]) -> Vec<Field> {
    let supported = match bytes {
        [0] => "not supported",
        [1] => "supported",
        _ => return Vec::new(),
    };
    vec![field("address resolution", supported)]
}

/// Heart Rate Measurement: a flags byte, then whatever the flags say is there.
///
/// The characteristic every BLE tutorial uses, and a good demonstration of why
/// a generic reading is not enough — the first byte is not part of the rate.
fn heart_rate_measurement(bytes: &[u8]) -> Vec<Field> {
    let [flags, rest @ ..] = bytes else {
        return Vec::new();
    };
    let wide = flags & 0x01 != 0;
    let mut out = Vec::new();

    let (rate, rest) = if wide {
        let [low, high, rest @ ..] = rest else {
            return Vec::new();
        };
        (u16::from_le_bytes([*low, *high]), rest)
    } else {
        let [value, rest @ ..] = rest else {
            return Vec::new();
        };
        (u16::from(*value), rest)
    };
    out.push(field("heart rate", format!("{rate} bpm")));

    // Bit 1 is whether contact is detected, bit 2 whether the sensor supports
    // saying so at all. Reporting "no contact" from a sensor that cannot tell
    // would be inventing a fact.
    if flags & 0x04 != 0 {
        out.push(field(
            "contact",
            if flags & 0x02 != 0 {
                "detected"
            } else {
                "not detected"
            },
        ));
    }

    let rest = if flags & 0x08 != 0 {
        let [low, high, rest @ ..] = rest else {
            return out;
        };
        out.push(field(
            "energy",
            format!("{} kJ", u16::from_le_bytes([*low, *high])),
        ));
        rest
    } else {
        rest
    };

    if flags & 0x10 != 0 {
        // RR intervals, each a uint16 in units of 1/1024 second.
        let intervals: Vec<String> = rest
            .chunks_exact(2)
            .map(|pair| {
                let raw = u16::from_le_bytes([pair[0], pair[1]]);
                format!("{:.0} ms", f32::from(raw) * 1000.0 / 1024.0)
            })
            .collect();
        if !intervals.is_empty() {
            out.push(field("RR intervals", intervals.join(", ")));
        }
    }
    out
}

fn appearance(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let raw = u16::from_le_bytes([*low, *high]);
    // The top ten bits are the category, the bottom six a subcategory.
    let category = raw >> 6;
    let name = match category {
        0 => "Unknown",
        1 => "Phone",
        2 => "Computer",
        3 => "Watch",
        4 => "Clock",
        5 => "Display",
        6 => "Remote Control",
        7 => "Eye-glasses",
        8 => "Tag",
        9 => "Keyring",
        10 => "Media Player",
        11 => "Barcode Scanner",
        12 => "Thermometer",
        13 => "Heart Rate Sensor",
        14 => "Blood Pressure",
        15 => "Human Interface Device",
        16 => "Glucose Meter",
        17 => "Running Walking Sensor",
        18 => "Cycling",
        19 => "Control Device",
        20 => "Network Device",
        21 => "Sensor",
        22 => "Light Fixtures",
        23 => "Fan",
        24 => "HVAC",
        25 => "Air Conditioning",
        26 => "Humidifier",
        27 => "Heating",
        28 => "Access Control",
        29 => "Motorized Device",
        30 => "Power Device",
        31 => "Light Source",
        32 => "Window Covering",
        33 => "Audio Sink",
        34 => "Audio Source",
        35 => "Motorized Vehicle",
        36 => "Domestic Appliance",
        37 => "Wearable Audio Device",
        38 => "Aircraft",
        39 => "AV Equipment",
        40 => "Display Equipment",
        41 => "Hearing Aid",
        42 => "Gaming",
        43 => "Signage",
        49 => "Pulse Oximeter",
        50 => "Weight Scale",
        51 => "Personal Mobility Device",
        52 => "Continuous Glucose Monitor",
        53 => "Insulin Pump",
        54 => "Medication Delivery",
        55 => "Spirometer",
        81 => "Outdoor Sports Activity",
        _ => "",
    };
    // The bottom six bits are a subcategory, which for the handful of
    // categories that define useful ones is the more specific answer.
    let subcategory = (raw & 0x3F) as u8;
    let detail = match (category, subcategory) {
        (_, 0) => "",
        (3, 1) => "Sports Watch",
        (3, 2) => "Smartwatch",
        (13, 1) => "Heart Rate Belt",
        (14, 1) => "Arm Blood Pressure",
        (14, 2) => "Wrist Blood Pressure",
        (15, 1) => "Keyboard",
        (15, 2) => "Mouse",
        (15, 3) => "Joystick",
        (15, 4) => "Gamepad",
        (15, 5) => "Digitizer Tablet",
        (15, 6) => "Card Reader",
        (15, 7) => "Digital Pen",
        (15, 8) => "Barcode Scanner",
        (17, 1) => "In-Shoe",
        (17, 2) => "On-Shoe",
        (17, 3) => "On-Hip",
        (18, 1) => "Cycling Computer",
        (18, 2) => "Speed Sensor",
        (18, 3) => "Cadence Sensor",
        (18, 4) => "Power Sensor",
        (18, 5) => "Speed and Cadence Sensor",
        (31, 1) => "Light Bulb",
        (31, 2) => "Light Strip",
        (31, 3) => "Lamp",
        (31, 4) => "Spotlight",
        (37, 1) => "Earbud",
        (37, 2) => "Headset",
        (37, 3) => "Headphones",
        (37, 4) => "Neck Band",
        (49, 1) => "Fingertip Pulse Oximeter",
        (49, 2) => "Wrist Worn Pulse Oximeter",
        _ => "",
    };

    vec![field(
        "appearance",
        match (name.is_empty(), detail.is_empty()) {
            (true, _) => format!("0x{raw:04X} — category {category}"),
            (false, true) => format!("{name} (0x{raw:04X})"),
            (false, false) => format!("{name} · {detail} (0x{raw:04X})"),
        },
    )]
}

/// Peripheral Preferred Connection Parameters: four uint16s.
fn preferred_connection_parameters(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c, d, e, f, g, h] = bytes else {
        return Vec::new();
    };
    let min = u16::from_le_bytes([*a, *b]);
    let max = u16::from_le_bytes([*c, *d]);
    let latency = u16::from_le_bytes([*e, *f]);
    let timeout = u16::from_le_bytes([*g, *h]);
    vec![
        field(
            "interval",
            // Both units, because a datasheet states the raw number and a human
            // wants the time.
            format!(
                "{:.2}–{:.2} ms ({min}–{max})",
                f32::from(min) * 1.25,
                f32::from(max) * 1.25
            ),
        ),
        field("latency", latency.to_string()),
        field(
            "timeout",
            format!("{} ms ({timeout})", u32::from(timeout) * 10),
        ),
    ]
}

/// PnP ID: a vendor id source, a vendor id, a product id and a version.
fn pnp_id(bytes: &[u8]) -> Vec<Field> {
    let [source, v0, v1, p0, p1, r0, r1] = bytes else {
        return Vec::new();
    };
    let vendor = u16::from_le_bytes([*v0, *v1]);
    let product = u16::from_le_bytes([*p0, *p1]);
    let revision = u16::from_le_bytes([*r0, *r1]);
    let source_name = match source {
        1 => "Bluetooth SIG",
        2 => "USB-IF",
        _ => "reserved",
    };
    vec![
        field("vendor id source", source_name),
        field(
            "vendor",
            match source {
                1 => crate::names::company(vendor).map_or_else(
                    || format!("0x{vendor:04X}"),
                    |name| format!("{name} (0x{vendor:04X})"),
                ),
                _ => format!("0x{vendor:04X}"),
            },
        ),
        field("product", format!("0x{product:04X}")),
        // "The value shall be 0xJJMN for version JJ.M.N."
        field(
            "version",
            format!(
                "{}.{}.{}",
                revision >> 8,
                (revision >> 4) & 0xF,
                revision & 0xF
            ),
        ),
    ]
}

/// System ID: a 40-bit manufacturer identifier and a 24-bit OUI.
fn system_id(bytes: &[u8]) -> Vec<Field> {
    let [m0, m1, m2, m3, m4, o0, o1, o2] = bytes else {
        return Vec::new();
    };
    vec![
        field(
            "manufacturer id",
            format!("{m4:02X}{m3:02X}{m2:02X}{m1:02X}{m0:02X}"),
        ),
        field("OUI", format!("{o2:02X}-{o1:02X}-{o0:02X}")),
    ]
}

fn temperature(bytes: &[u8]) -> Vec<Field> {
    // sint16, in hundredths of a degree. Signed matters: a freezer reads as a
    // large positive number if it is not.
    scaled_i16(bytes, 0.01, "°C", "temperature")
}

fn humidity(bytes: &[u8]) -> Vec<Field> {
    let [low, high] = bytes else {
        return Vec::new();
    };
    let raw = u16::from_le_bytes([*low, *high]);
    vec![field(
        "humidity",
        format!("{:.2} %", f32::from(raw) / 100.0),
    )]
}

fn pressure(bytes: &[u8]) -> Vec<Field> {
    let [a, b, c, d] = bytes else {
        return Vec::new();
    };
    // uint32, in tenths of a pascal.
    let raw = u32::from_le_bytes([*a, *b, *c, *d]);
    vec![field(
        "pressure",
        format!(
            "{:.1} Pa ({:.2} hPa)",
            raw as f64 / 10.0,
            raw as f64 / 1000.0
        ),
    )]
}

/// Date Time: year, month, day, hours, minutes, seconds.
fn date_time(bytes: &[u8]) -> Option<String> {
    let [y0, y1, month, day, hours, minutes, seconds] = bytes.get(..7)? else {
        return None;
    };
    let year = u16::from_le_bytes([*y0, *y1]);
    // Zero means "not known" for each field, which is part of the format
    // rather than a decoding failure.
    Some(format!(
        "{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{seconds:02}"
    ))
}

/// Temperature Measurement: flags, then an IEEE-11073 32-bit float.
fn temperature_measurement(bytes: &[u8]) -> Vec<Field> {
    let [flags, a, b, c, d, rest @ ..] = bytes else {
        return Vec::new();
    };
    let Some(value) = ieee11073_float32(u32::from_le_bytes([*a, *b, *c, *d])) else {
        return vec![field("temperature", "not a number")];
    };
    let unit = if flags & 0x01 != 0 { "°F" } else { "°C" };
    let mut out = vec![field("temperature", format!("{value:.2} {unit}"))];

    if flags & 0x02 != 0 {
        if let Some(when) = date_time(rest) {
            out.push(field("taken", when));
        }
    }
    out
}

/// IEEE-11073 32-bit FLOAT: an 8-bit exponent and a 24-bit signed mantissa.
///
/// `None` for the reserved values — NaN, infinities and "not at this
/// resolution" — which are statements about the reading, not numbers.
fn ieee11073_float32(raw: u32) -> Option<f64> {
    let exponent = (raw >> 24) as i8;
    let mantissa_raw = raw & 0x00FF_FFFF;
    // Sign-extend the 24-bit mantissa.
    let mantissa = if mantissa_raw & 0x0080_0000 != 0 {
        (mantissa_raw | 0xFF00_0000) as i32
    } else {
        mantissa_raw as i32
    };
    match mantissa {
        0x007F_FFFF | -0x0080_0000 | 0x0080_0000 | 0x0080_0001 | 0x0080_0002 => None,
        _ => Some(f64::from(mantissa) * 10f64.powi(i32::from(exponent))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(uuid: u16, bytes: &[u8]) -> Vec<Field> {
        characteristic(&BluetoothUuid::from_u16(uuid), bytes)
    }

    fn value_of(fields: &[Field], name: &str) -> Option<String> {
        fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.clone())
    }

    #[test]
    fn battery_level_is_a_percentage_and_says_when_it_is_not() {
        assert_eq!(value_of(&read(0x2A19, &[87]), "battery").unwrap(), "87 %");
        assert_eq!(value_of(&read(0x2A19, &[0]), "battery").unwrap(), "0 %");
        assert_eq!(value_of(&read(0x2A19, &[100]), "battery").unwrap(), "100 %");
        // Above 100 is out of range, and printing "200 %" would be a lie.
        assert!(value_of(&read(0x2A19, &[200]), "battery")
            .unwrap()
            .contains("out of range"));
        // Wrong length: say nothing rather than guess.
        assert!(read(0x2A19, &[]).is_empty());
        assert!(read(0x2A19, &[50, 50]).is_empty());
    }

    /// The first byte is flags, not the rate — the exact thing a generic
    /// reading gets wrong.
    #[test]
    fn heart_rate_reads_its_flags_before_its_value() {
        // Flags 0: 8-bit rate, no contact support, no energy, no RR.
        let simple = read(0x2A37, &[0x00, 60]);
        assert_eq!(value_of(&simple, "heart rate").unwrap(), "60 bpm");
        assert_eq!(simple.len(), 1, "nothing else was present");

        // Flags 1: 16-bit rate.
        let wide = read(0x2A37, &[0x01, 0x2C, 0x01]);
        assert_eq!(value_of(&wide, "heart rate").unwrap(), "300 bpm");

        // Flags 0x06: contact supported and detected.
        let contact = read(0x2A37, &[0x06, 70]);
        assert_eq!(value_of(&contact, "contact").unwrap(), "detected");
        // Flags 0x04: supported, not detected.
        let no_contact = read(0x2A37, &[0x04, 70]);
        assert_eq!(value_of(&no_contact, "contact").unwrap(), "not detected");
        // Flags 0x00: the sensor cannot tell, so nothing is claimed.
        assert!(value_of(&read(0x2A37, &[0x00, 70]), "contact").is_none());
    }

    #[test]
    fn heart_rate_reads_energy_and_rr_intervals() {
        // Flags 0x18: energy present, then RR intervals.
        // 8-bit rate 60, energy 500, one RR of 1024 (= 1000 ms).
        let full = read(0x2A37, &[0x18, 60, 0xF4, 0x01, 0x00, 0x04]);
        assert_eq!(value_of(&full, "heart rate").unwrap(), "60 bpm");
        assert_eq!(value_of(&full, "energy").unwrap(), "500 kJ");
        assert_eq!(value_of(&full, "RR intervals").unwrap(), "1000 ms");

        // A truncated packet gives up what it has rather than inventing more.
        let truncated = read(0x2A37, &[0x18, 60, 0xF4]);
        assert_eq!(value_of(&truncated, "heart rate").unwrap(), "60 bpm");
        assert!(value_of(&truncated, "energy").is_none());
    }

    #[test]
    fn appearance_splits_category_from_subcategory() {
        // 833 = 0x0341: category 13 (Heart Rate Sensor), subcategory 1.
        let sensor = read(0x2A01, &[0x41, 0x03]);
        assert!(value_of(&sensor, "appearance")
            .unwrap()
            .starts_with("Heart Rate Sensor"));
        // 64 = category 1, Phone.
        assert!(read(0x2A01, &[0x40, 0x00])[0].value.starts_with("Phone"));
        // A subcategory is the more specific answer where one is defined.
        // 0x03C1 is category 15 (HID), subcategory 1 (Keyboard).
        let keyboard = read(0x2A01, &[0xC1, 0x03]);
        assert!(
            keyboard[0].value.contains("Keyboard"),
            "{}",
            keyboard[0].value
        );
        // An unknown category falls back to the number rather than guessing.
        assert!(read(0x2A01, &[0x00, 0xFF])[0].value.contains("category"));
        assert!(read(0x2A01, &[0x41]).is_empty());
    }

    #[test]
    fn connection_parameters_come_with_their_units() {
        // 6..12 units, latency 0, timeout 500.
        let params = read(0x2A04, &[0x06, 0x00, 0x0C, 0x00, 0x00, 0x00, 0xF4, 0x01]);
        let interval = value_of(&params, "interval").unwrap();
        assert!(interval.contains("7.50"), "{interval}");
        assert!(interval.contains("15.00"), "{interval}");
        assert_eq!(value_of(&params, "latency").unwrap(), "0");
        assert!(value_of(&params, "timeout").unwrap().contains("5000 ms"));
    }

    #[test]
    fn pnp_id_reads_the_packed_version() {
        // Source 2 (USB-IF), vendor 0x1915, product 0x1234, version 0x0123.
        let pnp = read(0x2A50, &[0x02, 0x15, 0x19, 0x34, 0x12, 0x23, 0x01]);
        assert_eq!(value_of(&pnp, "vendor id source").unwrap(), "USB-IF");
        assert_eq!(value_of(&pnp, "product").unwrap(), "0x1234");
        // 0xJJMN is version JJ.M.N.
        assert_eq!(value_of(&pnp, "version").unwrap(), "1.2.3");
        assert!(read(0x2A50, &[0x02, 0x15]).is_empty());
    }

    #[test]
    fn a_system_id_is_read_back_the_way_it_is_written() {
        let id = read(0x2A23, &[0x01, 0x02, 0x03, 0x04, 0x05, 0xAA, 0xBB, 0xCC]);
        assert_eq!(value_of(&id, "manufacturer id").unwrap(), "0504030201");
        assert_eq!(value_of(&id, "OUI").unwrap(), "CC-BB-AA");
    }

    #[test]
    fn environmental_values_carry_their_scale() {
        // 2350 hundredths = 23.50 °C, and the type is signed.
        assert_eq!(
            value_of(&read(0x2A6E, &[0x2E, 0x09]), "temperature").unwrap(),
            "23.50 °C"
        );
        assert_eq!(
            value_of(&read(0x2A6E, &[0xD2, 0xFE]), "temperature").unwrap(),
            "-3.02 °C"
        );
        assert_eq!(
            value_of(&read(0x2A6F, &[0x10, 0x27]), "humidity").unwrap(),
            "100.00 %"
        );
        assert!(
            value_of(&read(0x2A6D, &[0x40, 0x42, 0x0F, 0x00]), "pressure")
                .unwrap()
                .contains("1000.00 hPa")
        );
    }

    /// IEEE-11073 is not IEEE-754, and treating the top byte as part of the
    /// mantissa gives a number that looks plausible and is wrong.
    #[test]
    fn temperature_measurement_reads_a_medical_float() {
        // Exponent -2, mantissa 3725 → 37.25 °C. Flags 0: Celsius.
        let celsius = read(0x2A1C, &[0x00, 0x8D, 0x0E, 0x00, 0xFE]);
        assert_eq!(value_of(&celsius, "temperature").unwrap(), "37.25 °C");

        // Flags bit 0 set: the same number, Fahrenheit.
        let fahrenheit = read(0x2A1C, &[0x01, 0x8D, 0x0E, 0x00, 0xFE]);
        assert!(value_of(&fahrenheit, "temperature")
            .unwrap()
            .ends_with("°F"));

        // A negative mantissa sign-extends.
        let below = read(0x2A1C, &[0x00, 0x73, 0xF1, 0xFF, 0xFE]);
        assert_eq!(value_of(&below, "temperature").unwrap(), "-37.25 °C");

        // The reserved NaN mantissa is a statement, not a number.
        assert_eq!(
            value_of(
                &read(0x2A1C, &[0x00, 0xFF, 0xFF, 0x7F, 0x00]),
                "temperature"
            )
            .unwrap(),
            "not a number"
        );
    }

    #[test]
    fn temperature_measurement_reads_its_optional_timestamp() {
        // Flags 0x02: a Date Time follows the value.
        let stamped = read(
            0x2A1C,
            &[
                0x02, 0x8D, 0x0E, 0x00, 0xFE, 0xE8, 0x07, 0x09, 0x1B, 0x0A, 0x1E, 0x00,
            ],
        );
        assert_eq!(value_of(&stamped, "taken").unwrap(), "2024-09-27 10:30:00");
    }

    /// The time cluster builds on itself: Current Time is an Exact Time 256 is
    /// a Day Date Time is a Date Time. Getting the nesting wrong shifts every
    /// field after it.
    #[test]
    fn the_time_cluster_nests_correctly() {
        // 2024-09-27 10:30:00, Friday, 128/256 s, adjusted manually.
        let full = [0xE8, 0x07, 0x09, 0x1B, 0x0A, 0x1E, 0x00, 0x05, 0x80, 0x01];

        let date = read(0x2A08, &full[..7]);
        assert_eq!(value_of(&date, "date").unwrap(), "2024-09-27 10:30:00");

        let day = read(0x2A0A, &full[..8]);
        assert_eq!(value_of(&day, "day").unwrap(), "Friday");

        let exact = read(0x2A0C, &full[..9]);
        assert_eq!(value_of(&exact, "fraction").unwrap(), "0.500 s");

        let current = read(0x2A2B, &full);
        assert_eq!(value_of(&current, "date").unwrap(), "2024-09-27 10:30:00");
        assert_eq!(value_of(&current, "day").unwrap(), "Friday");
        assert_eq!(
            value_of(&current, "adjust reason").unwrap(),
            "manual update"
        );
    }

    #[test]
    fn a_time_zone_is_quarter_hours_and_signed() {
        assert_eq!(
            value_of(&read(0x2A0E, &[0]), "time zone").unwrap(),
            "UTC+00:00"
        );
        // +5:30 is 22 quarter-hours — the case that catches a whole-hour
        // assumption.
        assert_eq!(
            value_of(&read(0x2A0E, &[22]), "time zone").unwrap(),
            "UTC+05:30"
        );
        // -8:00 is -32, which must sign-extend.
        assert_eq!(
            value_of(&read(0x2A0E, &[0xE0]), "time zone").unwrap(),
            "UTC-08:00"
        );
        assert_eq!(
            value_of(&read(0x2A0E, &[0x80]), "time zone").unwrap(),
            "unknown"
        );
    }

    /// Blood pressure uses 16-bit SFLOATs, which are not 16-bit integers and
    /// not IEEE-754 halves.
    #[test]
    fn blood_pressure_reads_medical_floats() {
        // Exponent 0, mantissas 120 / 80 / 93, in mmHg.
        let reading = [0x00, 0x78, 0x00, 0x50, 0x00, 0x5D, 0x00];
        let bp = read(0x2A35, &reading);
        assert_eq!(value_of(&bp, "systolic").unwrap(), "120.0 mmHg");
        assert_eq!(value_of(&bp, "diastolic").unwrap(), "80.0 mmHg");
        assert_eq!(value_of(&bp, "mean arterial").unwrap(), "93.0 mmHg");

        // Flags bit 0 switches the unit without changing the numbers.
        let kpa = read(0x2A36, &[0x01, 0x78, 0x00, 0x50, 0x00, 0x5D, 0x00]);
        assert!(value_of(&kpa, "systolic").unwrap().ends_with("kPa"));
    }

    #[test]
    fn blood_pressure_reads_its_optional_pulse_after_its_optional_timestamp() {
        // Flags 0x06: timestamp present, then pulse.
        let mut reading = vec![0x06, 0x78, 0x00, 0x50, 0x00, 0x5D, 0x00];
        reading.extend_from_slice(&[0xE8, 0x07, 0x09, 0x1B, 0x0A, 0x1E, 0x00]);
        reading.extend_from_slice(&[0x48, 0x00]); // 72 bpm
        let bp = read(0x2A35, &reading);
        assert_eq!(value_of(&bp, "taken").unwrap(), "2024-09-27 10:30:00");
        assert_eq!(value_of(&bp, "pulse").unwrap(), "72 bpm");
    }

    #[test]
    fn running_and_cycling_measurements_apply_their_scales() {
        // Speed 2.5 m/s is 640 in 1/256 units; cadence 180; running.
        let rsc = read(0x2A53, &[0x04, 0x80, 0x02, 180]);
        assert_eq!(value_of(&rsc, "speed").unwrap(), "2.50 m/s");
        assert_eq!(value_of(&rsc, "cadence").unwrap(), "180 steps/min");
        assert_eq!(value_of(&rsc, "moving").unwrap(), "running");
        // Bit 2 clear is walking, which is a different fact.
        assert_eq!(
            value_of(&read(0x2A53, &[0x00, 0x80, 0x02, 60]), "moving").unwrap(),
            "walking"
        );

        // CSC with wheel data only: 1000 revolutions, event at 1024/1024 s.
        let csc = read(0x2A5B, &[0x01, 0xE8, 0x03, 0x00, 0x00, 0x00, 0x04]);
        assert_eq!(value_of(&csc, "wheel revolutions").unwrap(), "1000");
        assert_eq!(value_of(&csc, "last wheel event").unwrap(), "1.000 s");
        assert!(value_of(&csc, "crank revolutions").is_none());
    }

    #[test]
    fn weight_uses_a_different_scale_per_unit() {
        // Metric: 5 g steps. 14000 × 0.005 = 70 kg.
        assert_eq!(
            value_of(&read(0x2A9D, &[0x00, 0xB0, 0x36]), "weight").unwrap(),
            "70.00 kg"
        );
        // Imperial: hundredths of a pound.
        assert!(value_of(&read(0x2A9D, &[0x01, 0xB0, 0x36]), "weight")
            .unwrap()
            .ends_with("lb"));
    }

    #[test]
    fn elevation_is_a_signed_24_bit_value() {
        // 12345 cm = 123.45 m.
        assert_eq!(
            value_of(&read(0x2A6C, &[0x39, 0x30, 0x00]), "elevation").unwrap(),
            "123.45 m"
        );
        // Below sea level must sign-extend rather than read as 16 megametres.
        let below = value_of(&read(0x2A6C, &[0xC7, 0xCF, 0xFF]), "elevation").unwrap();
        assert!(below.starts_with('-'), "{below}");
    }

    #[test]
    fn alerts_name_their_categories() {
        assert_eq!(value_of(&read(0x2A43, &[3]), "category").unwrap(), "Call");
        let unread = read(0x2A45, &[1, 7]);
        assert_eq!(value_of(&unread, "category").unwrap(), "Email");
        assert_eq!(value_of(&unread, "unread").unwrap(), "7");

        let mut alert = vec![5, 2];
        alert.extend_from_slice(b"Alice");
        let new = read(0x2A46, &alert);
        assert_eq!(value_of(&new, "category").unwrap(), "SMS or MMS");
        assert_eq!(value_of(&new, "text").unwrap(), "Alice");
    }

    /// Descriptors are small and their encodings are short, which is exactly
    /// why leaving them as hex is annoying.
    #[test]
    fn descriptors_read_their_bits() {
        let cccd = |bytes: &[u8]| {
            descriptor(&BluetoothUuid::from_u16(0x2902), bytes)[0]
                .value
                .clone()
        };
        assert_eq!(cccd(&[0x01, 0x00]), "notifications");
        assert_eq!(cccd(&[0x02, 0x00]), "indications");
        assert_eq!(cccd(&[0x03, 0x00]), "notifications and indications");
        assert_eq!(cccd(&[0x00, 0x00]), "nothing");

        let extended = descriptor(&BluetoothUuid::from_u16(0x2900), &[0x01, 0x00]);
        assert_eq!(extended[0].value, "reliable write");

        // User Description is free text.
        let described = descriptor(&BluetoothUuid::from_u16(0x2901), b"Left motor");
        assert_eq!(described[0].value, "Left motor");

        // An unknown descriptor says nothing rather than guessing.
        assert!(descriptor(&BluetoothUuid::from_u16(0x2905), &[1, 2]).is_empty());
    }

    #[test]
    fn a_presentation_format_says_how_to_read_its_characteristic() {
        // uint16, exponent -2, unit percent, SIG namespace.
        let format = descriptor(
            &BluetoothUuid::from_u16(0x2904),
            &[0x06, 0xFE, 0xAD, 0x27, 0x01, 0x00, 0x00],
        );
        assert_eq!(value_of(&format, "format").unwrap(), "uint16");
        assert_eq!(value_of(&format, "exponent").unwrap(), "-2");
        assert_eq!(value_of(&format, "unit").unwrap(), "percent");
        assert_eq!(value_of(&format, "namespace").unwrap(), "Bluetooth SIG");
    }

    /// 0x2A70 is True Wind *Speed* and 0x2A71 is True Wind *Direction*. They
    /// are adjacent, both `uint16`, both scaled by a hundredth — and reading
    /// one as the other gives a number that looks entirely plausible.
    #[test]
    fn wind_speed_and_direction_are_not_the_same_characteristic() {
        let raw = [0x10, 0x27]; // 10000
        assert_eq!(
            value_of(&read(0x2A70, &raw), "wind speed").unwrap(),
            "100.00 m/s"
        );
        assert_eq!(
            value_of(&read(0x2A71, &raw), "wind direction").unwrap(),
            "100.00 °"
        );
        // And the apparent pair the same way round.
        assert!(value_of(&read(0x2A72, &raw), "wind speed").is_some());
        assert!(value_of(&read(0x2A73, &raw), "wind direction").is_some());
    }

    #[test]
    fn feature_bitfields_name_the_bits_that_are_set() {
        // Blood pressure: body movement + irregular pulse.
        let bp = read(0x2A49, &[0x05, 0x00]);
        let supports = value_of(&bp, "supports").unwrap();
        assert!(supports.contains("body movement detection"), "{supports}");
        assert!(supports.contains("irregular pulse detection"), "{supports}");
        assert!(!supports.contains("cuff fit"), "{supports}");

        // Nothing set says so, rather than printing an empty string.
        assert_eq!(
            value_of(&read(0x2A49, &[0x00, 0x00]), "supports").unwrap(),
            "none"
        );

        // CSC: wheel and crank.
        let csc = value_of(&read(0x2A5C, &[0x03, 0x00]), "supports").unwrap();
        assert!(csc.contains("wheel revolutions") && csc.contains("crank revolutions"));
    }

    #[test]
    fn an_alert_category_bitmask_names_its_categories() {
        // Bits 1 and 3: Email and Call.
        let mask = read(0x2A47, &[0x0A, 0x00]);
        let categories = value_of(&mask, "categories").unwrap();
        assert!(categories.contains("Email") && categories.contains("Call"));
        assert!(!categories.contains("News"));
        assert_eq!(
            value_of(&read(0x2A47, &[0x00]), "categories").unwrap(),
            "none"
        );
    }

    #[test]
    fn reference_time_reports_its_sentinels() {
        // GPS, accuracy 8 units (1 s), 0 days 2 hours.
        let known = read(0x2A14, &[0x02, 0x08, 0x00, 0x02]);
        assert_eq!(value_of(&known, "time source").unwrap(), "GPS");
        assert_eq!(value_of(&known, "accuracy").unwrap(), "±1.000 s");
        assert_eq!(value_of(&known, "since update").unwrap(), "0 d 2 h");

        let unknown = read(0x2A14, &[0x02, 0xFF, 0xFF, 0xFF]);
        assert_eq!(value_of(&unknown, "accuracy").unwrap(), "unknown");
        assert_eq!(
            value_of(&unknown, "since update").unwrap(),
            "255 days or more"
        );
    }

    #[test]
    fn fitness_machine_ranges_and_status_read_out() {
        let speed = read(0x2AD4, &[0x0A, 0x00, 0xE8, 0x03, 0x01, 0x00]);
        assert_eq!(value_of(&speed, "minimum").unwrap(), "10");
        assert_eq!(value_of(&speed, "maximum").unwrap(), "1000");
        assert_eq!(value_of(&speed, "step").unwrap(), "1");

        let heart = read(0x2AD7, &[40, 200, 1]);
        assert_eq!(value_of(&heart, "minimum").unwrap(), "40 bpm");

        // Resistance is signed and scaled, unlike the others.
        let resistance = read(0x2AD6, &[0xF6, 0xFF, 0x64, 0x00, 0x0A, 0x00]);
        assert_eq!(value_of(&resistance, "minimum").unwrap(), "-1.0");
        assert_eq!(value_of(&resistance, "maximum").unwrap(), "10.0");

        let status = read(0x2AD3, &[0x00, 0x04]);
        assert_eq!(
            value_of(&status, "status").unwrap(),
            "High Intensity Interval"
        );
    }

    /// A CGM value can pack several records. The leading size byte is how you
    /// tell, and ignoring it reads the second record as part of the first.
    #[test]
    fn a_cgm_measurement_notices_when_more_records_follow() {
        // One record of six bytes: size, flags, SFLOAT 100, offset 5.
        let single = [0x06, 0x00, 0x64, 0x00, 0x05, 0x00];
        let one = read(0x2AA7, &single);
        assert_eq!(value_of(&one, "glucose").unwrap(), "100.0 mg/dL");
        assert_eq!(value_of(&one, "time offset").unwrap(), "5 min");
        assert!(value_of(&one, "record").is_none(), "nothing follows");

        // The same record, with another appended.
        let mut packed = single.to_vec();
        packed.extend_from_slice(&single);
        let many = read(0x2AA7, &packed);
        assert!(value_of(&many, "record").unwrap().contains("more records"));
    }

    #[test]
    fn the_user_data_service_reads_a_profile() {
        assert_eq!(
            value_of(&read(0x2A85, &[0xD0, 0x07, 0x06, 0x0F]), "born").unwrap(),
            "2000-06-15"
        );
        assert_eq!(
            value_of(&read(0x2A8D, &[190]), "max heart rate").unwrap(),
            "190 bpm"
        );
        assert_eq!(
            value_of(&read(0x2A96, &[48]), "VO₂ max").unwrap(),
            "48 ml/kg/min"
        );
        // Circumferences are metres in hundredths, like height.
        assert_eq!(
            value_of(&read(0x2A97, &[0x50, 0x00]), "waist circumference").unwrap(),
            "0.80 m"
        );
        assert_eq!(value_of(&read(0x2A8A, b"Ada"), "text").unwrap(), "Ada");

        // Five zones means four boundaries, not five.
        let zones = read(0x2A8B, &[110, 130, 150, 170]);
        assert_eq!(zones.len(), 4);
        assert_eq!(value_of(&zones, "hard / maximum").unwrap(), "170 bpm");
        assert!(read(0x2A8B, &[110, 130, 150]).is_empty());
    }

    #[test]
    fn scan_parameters_carry_their_units() {
        // 0x0010 = 16 units = 10 ms.
        let window = read(0x2A4F, &[0x10, 0x00, 0x10, 0x00]);
        assert!(value_of(&window, "interval").unwrap().contains("10.00 ms"));
        assert!(value_of(&window, "window").unwrap().contains("10.00 ms"));
    }

    #[test]
    fn a_cgm_session_start_carries_its_zone() {
        // 2024-09-27 10:30:00, UTC+01:00 (4 quarter-hours), daylight time.
        let value = [0xE8, 0x07, 0x09, 0x1B, 0x0A, 0x1E, 0x00, 0x04, 0x04];
        let start = read(0x2AAA, &value);
        assert_eq!(value_of(&start, "started").unwrap(), "2024-09-27 10:30:00");
        assert_eq!(value_of(&start, "time zone").unwrap(), "UTC+01:00");
        assert!(value_of(&start, "DST offset").unwrap().contains("daylight"));
    }

    #[test]
    fn device_information_strings_read_as_text() {
        assert_eq!(
            value_of(&read(0x2A29, b"Nordic Semiconductor"), "text").unwrap(),
            "Nordic Semiconductor"
        );
        assert_eq!(
            value_of(&read(0x2A00, b"Blinky"), "text").unwrap(),
            "Blinky"
        );
        // Not text: say nothing rather than print replacement characters.
        assert!(read(0x2A29, &[0xFF, 0xFE]).is_empty());
        assert!(read(0x2A29, &[]).is_empty());
    }

    #[test]
    fn enumerations_name_their_values_and_refuse_unknown_ones() {
        assert_eq!(value_of(&read(0x2A38, &[2]), "location").unwrap(), "Wrist");
        assert_eq!(
            value_of(&read(0x2A06, &[2]), "alert").unwrap(),
            "High Alert"
        );
        assert!(read(0x2A38, &[99]).is_empty(), "not a defined location");
        assert!(read(0x2A06, &[9]).is_empty(), "not a defined alert level");
    }

    /// A vendor characteristic is 128-bit, so it never collides with an
    /// assigned number — and a device publishing one is usually the reason you
    /// are looking at it.
    #[test]
    fn the_ancs_notification_source_reads_out() {
        let uuid = BluetoothUuid::parse("9fbf120d-6301-42d9-8c58-25e699a21dbd").unwrap();
        // Added, silent+important, Incoming Call, 1 in category, id 42.
        let value = [0x00, 0x03, 0x01, 0x01, 0x2A, 0x00, 0x00, 0x00];
        let fields = characteristic(&uuid, &value);

        assert_eq!(value_of(&fields, "event").unwrap(), "Added");
        assert_eq!(value_of(&fields, "category").unwrap(), "Incoming Call");
        assert_eq!(value_of(&fields, "notification id").unwrap(), "42");
        let flags = value_of(&fields, "flags").unwrap();
        assert!(
            flags.contains("silent") && flags.contains("important"),
            "{flags}"
        );

        // A neighbouring UUID is a different characteristic entirely.
        let neighbour = BluetoothUuid::parse("9fbf120e-6301-42d9-8c58-25e699a21dbd").unwrap();
        assert!(characteristic(&neighbour, &value).is_empty());
    }

    #[test]
    fn a_characteristic_nobody_has_defined_reads_as_nothing() {
        assert!(read(0x2A1F, &[1, 2, 3]).is_empty());
        let vendor = BluetoothUuid::parse("f000aa00-0451-4000-b000-000000000000").unwrap();
        assert!(characteristic(&vendor, &[1, 2, 3]).is_empty());
    }
}
