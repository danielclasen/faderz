//! The fader law of the faderz web UI (`web/src/lib/audio/db.ts`
//! `FADER_TAPER`), so a surface fader at a given height shows the dB the
//! screen shows at that height: 0 dB at three quarters of the travel,
//! linear in dB between the points.

/// Fully closed in the contract (`FaderDb`).
pub const OFF_DB: f64 = -90.0;
pub const MAX_DB: f64 = 10.0;

/// Fader travel as (position, dB), position 0 at the bottom, 1 at the top.
pub const TAPER: [(f64, f64); 10] = [
    (0.0, OFF_DB),
    (0.075, -60.0),
    (0.15, -40.0),
    (0.25, -30.0),
    (0.375, -20.0),
    (0.5, -10.0),
    (0.625, -5.0),
    (0.75, 0.0),
    (0.875, 5.0),
    (1.0, MAX_DB),
];

/// The largest value of a 14-bit MCU fader (pitch bend).
pub const FADER_MAX: u16 = 0x3fff;

fn interpolate(x: f64, from: fn(&(f64, f64)) -> f64, to: fn(&(f64, f64)) -> f64) -> f64 {
    let (first, last) = (&TAPER[0], &TAPER[TAPER.len() - 1]);
    if x <= from(first) {
        return to(first);
    }
    if x >= from(last) {
        return to(last);
    }
    for pair in TAPER.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if x <= from(b) {
            let t = (x - from(a)) / (from(b) - from(a));
            return to(a) + t * (to(b) - to(a));
        }
    }
    to(last)
}

/// Fader position (0..1) of a dB value.
pub fn db_to_position(db: f64) -> f64 {
    interpolate(db.clamp(OFF_DB, MAX_DB), |p| p.1, |p| p.0)
}

/// dB value of a fader position (0..1), rounded to 0.1 dB; the floor
/// snaps to off.
pub fn position_to_db(position: f64) -> f64 {
    let db = interpolate(position.clamp(0.0, 1.0), |p| p.0, |p| p.1);
    if db <= OFF_DB + 0.05 {
        return OFF_DB;
    }
    let rounded = (db * 10.0).round() / 10.0;
    if rounded == 0.0 { 0.0 } else { rounded }
}

/// The 14-bit fader value of a dB value.
pub fn db_to_fader(db: f64) -> u16 {
    (db_to_position(db) * f64::from(FADER_MAX)).round() as u16
}

/// The dB value of a 14-bit fader value.
pub fn fader_to_db(value: u16) -> f64 {
    position_to_db(f64::from(value.min(FADER_MAX)) / f64::from(FADER_MAX))
}

/// A fader's dB value as the scribble strip shows it, at most 7
/// characters: `-inf` when closed.
pub fn format_db(db: f64) -> String {
    if db <= OFF_DB {
        return "-inf".to_owned();
    }
    if db > 0.0 {
        format!("+{db:.1}")
    } else {
        format!("{db:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The points the web UI draws (0 dB at 0.75) map both ways.
    #[test]
    fn the_taper_points_map_both_ways() {
        for (position, db) in TAPER {
            assert!((db_to_position(db) - position).abs() < 1e-12, "{db}");
            assert_eq!(position_to_db(position), db, "{position}");
        }
        assert_eq!(db_to_position(-200.0), 0.0);
        assert_eq!(db_to_position(30.0), 1.0);
        assert_eq!(position_to_db(-1.0), OFF_DB);
    }

    #[test]
    fn positions_between_points_are_linear_in_db_and_rounded() {
        assert_eq!(position_to_db(0.8125), 2.5);
        assert_eq!(position_to_db(0.7), -2.0);
        assert!((db_to_position(-15.0) - 0.4375).abs() < 1e-12);
        // Just above the floor is still off: the web snaps within 0.05 dB.
        assert_eq!(position_to_db(0.0000001), OFF_DB);
    }

    /// The 14-bit fader resolves the top of the travel finer than 0.1 dB,
    /// so a fader set to 0 dB reads back 0 dB.
    #[test]
    fn a_fader_value_round_trips_through_db() {
        for db in [-90.0, -60.0, -30.0, -10.0, -5.0, 0.0, 0.5, 5.0, 10.0] {
            assert_eq!(fader_to_db(db_to_fader(db)), db, "{db}");
        }
        assert_eq!(db_to_fader(0.0), 12287);
        assert_eq!(db_to_fader(10.0), FADER_MAX);
        assert_eq!(db_to_fader(OFF_DB), 0);
    }

    #[test]
    fn the_strip_shows_db_in_seven_characters() {
        assert_eq!(format_db(-90.0), "-inf");
        assert_eq!(format_db(0.0), "0.0");
        assert_eq!(format_db(4.5), "+4.5");
        assert_eq!(format_db(-60.0), "-60.0");
        assert!(format_db(-89.9).len() <= 7);
    }
}
