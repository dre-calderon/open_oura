//! Activity score. Ported from ecore `get_activity_score_raw @ 0xd5788` (body
//! 0xd5788–0xd7300) and the combiner `get_activity_score_from_raw_100`.
//!
//! Seven contributors c0–c6 are each produced by the shared "band +
//! piecewise-linear map" idiom (score breakpoints `A = [0, 25, 95, 100]` in
//! rodata @ 0x6e9c0) and combined as
//! `25*c0 + 15*c1 + 10*c2 + 10*c3 + 15*c4 + 10*c5 + 15*c6` (÷100 for 0–100
//! contributors), rounded half up. A feature-gated secondary output at
//! `out+0xc` is `(10*c5 + 15*c6)/25` when the feature is on, else `(c5+c6)/2`.
//!
//! Day-record layout: stride 0x18 (24 B), 9 slots at state +0x00…+0xc0
//! (+0xc0 = most recent). Contributor B tables: c0 @ 0x6e8d0, c1 @ 0x6eed0,
//! c2 @ 0x6f140, c3 @ 0x6f200, c4 @ 0x6ece0, c5 @ 0x6f210, c6 @ 0x6e8d0+0x10.
//! c4 day weights: rodata @ 0x7bc28 = `[19, 42, 77, 109, 134, 153, 166]`
//! (sum 700).
//!
//! Contributor names: c0/c3/c4 use Oura's public contributor names
//! (Meet Daily Targets, Training Frequency, Training Volume). The names for
//! **c1, c2, c5 and c6 are INFERRED** from units/defaults/Java classes — the
//! mechanics and constants are proven, the semantics are best guesses
//! ("Inactivity Time", "Movement Every Hour", "Recovery Time",
//! "Inactivity streak").

/// One per-day activity record (stride 0x18 in `activity_score_state`
/// @ 0x211848 / `activity_score_stateless` @ 0x211920). Field offsets are the
/// ones the scorer reads; `field_10/12/14` keep their offsets as names because
/// their semantics were not recovered (see `actinfo_update_statistics`
/// @ 0xce570 — not yet analysed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityDayRecord {
    /// +0x00: equivalent distance (`ActivityHistoryInput` default 5000).
    pub eq_distance: u16,
    /// +0x02: distance target.
    pub eq_distance_target: u16,
    /// +0x04: sedentary minutes (c1; Java defaults 999/480).
    pub sedentary_minutes: u16,
    /// +0x06: moderate MET-minutes (c3/c4).
    pub moderate_met_minutes: u16,
    /// +0x08: vigorous MET-minutes (c3/c4).
    pub vigorous_met_minutes: u16,
    /// +0x0a: sit-alert count (c2, range 0–9+).
    pub sit_alerts: u8,
    /// +0x10: unknown, read by c6.
    pub field_10: u16,
    /// +0x12: inactivity-ish, read by c5/c6.
    pub field_12: u16,
    /// +0x14: inactivity-ish; c5 threshold >45, c6 >14.
    pub field_14: u16,
    /// +0x16: target status enum — 0 = unknown (compare distance), 1 = met,
    /// >1 = not met (`ACTIVITY_TARGET_STATUS_UNKNOWN = 0`).
    pub target_status: u8,
}

/// Activity scorer state: a 9-slot per-day record array (state +0x00…+0xc0,
/// stride 0x18), **most recent day last**. c6 walks all 9 slots
/// most-recent → oldest; c0/c3/c4/c5 use the last 7 (6 history + today).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActivityScoreState {
    /// `days[8]` is today (state +0xc0); `days[0]` is the oldest slot.
    pub days: [ActivityDayRecord; 9],
}

impl ActivityScoreState {
    /// The 7-day scoring window: 6 history slots + today (most recent last).
    pub fn window7(&self) -> [ActivityDayRecord; 7] {
        let d = &self.days;
        [d[2], d[3], d[4], d[5], d[6], d[7], d[8]]
    }
}

/// A day record carries data iff its field sum (incl. +0x14) ≠ 0.
/// [PROVEN — `cmn` overflow checks in `get_activity_score_raw`]
fn has_data(d: &ActivityDayRecord) -> bool {
    (d.eq_distance as u32)
        + (d.eq_distance_target as u32)
        + (d.sedentary_minutes as u32)
        + (d.moderate_met_minutes as u32)
        + (d.vigorous_met_minutes as u32)
        + (d.field_10 as u32)
        + (d.field_12 as u32)
        + (d.field_14 as u32)
        != 0
}

/// The shared band idiom [PROVEN, rodata @ 0x6e9c0]: with score breakpoints
/// `A = [0, 25, 95, 100]`, band `i` (1-based) maps `B[i-1] → A[i-1]` to
/// `upper[i] → A[i]` linearly. Bands may be ascending (lower = worse) or
/// descending (lower = better); values beyond the outer bounds extrapolate
/// the nearest band and the result is clamped to [0, 100].
pub fn band_map(value: f64, b: [f64; 4], uppers: [f64; 3]) -> f64 {
    const A: [f64; 4] = [0.0, 25.0, 95.0, 100.0];
    let band = |i: usize| A[i] + (A[i + 1] - A[i]) * (value - b[i]) / (uppers[i] - b[i]);
    let in_band = |i: usize| {
        let (lo, hi) = if b[i] <= uppers[i] {
            (b[i], uppers[i])
        } else {
            (uppers[i], b[i])
        };
        value >= lo && value <= hi
    };
    let out = if in_band(0) {
        band(0)
    } else if in_band(1) {
        band(1)
    } else if in_band(2) {
        band(2)
    } else if (value - b[0]) * (uppers[0] - b[0]) < 0.0 {
        // Beyond the B[0] end (above an ascending table, below a descending
        // one): extrapolate band 1 — the clamp turns it into 0.
        band(0)
    } else {
        // Beyond the B[3] end: extrapolate band 3 — the clamp turns it into
        // 100.
        band(2)
    };
    out.clamp(0.0, 100.0)
}

/// c0 — Meet Daily Targets. `hits` = days in the last 7 (6 history + today)
/// where `targetStatus == 1`, or (`status == 0` and
/// `eqDistance >= eqDistanceTarget`). Ratio `r = 7 * hits / days_with_data`
/// (×1/7 = 0x3E124925). Onboarding rule: with no history (`hits == 0 &&
/// days <= 1`) today's miss counts as a hit. B @ 0x6e8d0 = [0, 1, 5, 6],
/// uppers 1/5/6.
pub fn c0_meet_daily_targets(history: &[ActivityDayRecord; 6], today: &ActivityDayRecord) -> f64 {
    let mut hits = 0u32;
    let mut days = 0u32;
    for d in history.iter().chain(std::iter::once(today)) {
        if !has_data(d) {
            continue;
        }
        days += 1;
        if d.target_status == 1 || (d.target_status == 0 && d.eq_distance >= d.eq_distance_target)
        {
            hits += 1;
        }
    }
    if hits == 0 && days <= 1 {
        hits = 1;
        days = days.max(1);
    }
    let r = 7.0 * hits as f64 / days as f64;
    band_map(r, [0.0, 1.0, 5.0, 6.0], [1.0, 5.0, 6.0])
}

/// c1 — "Inactivity Time" (name INFERRED; numbers PROVEN). Today's sedentary
/// minutes, lower = better. B @ 0x6eed0 = [1020, 955, 400, 300]: 1020 → 0,
/// 955 → 25, 400 → 95, ≤300 capped 100, >1020 → 0.
pub fn c1_inactivity_time(today: &ActivityDayRecord) -> f64 {
    band_map(
        today.sedentary_minutes as f64,
        [1020.0, 955.0, 400.0, 300.0],
        [955.0, 400.0, 300.0],
    )
}

/// c2 — "Movement Every Hour" (name INFERRED; numbers PROVEN). Today's
/// sit-alert count, lower = better. B @ 0x6f140 = [10, 5, 1, 0]: 10 → 0,
/// 5 → 25, 1 → 95, 0 → 100, >9 → 0.
pub fn c2_movement_every_hour(today: &ActivityDayRecord) -> f64 {
    band_map(today.sit_alerts as f64, [10.0, 5.0, 1.0, 0.0], [5.0, 1.0, 0.0])
}

/// c3 — Training Frequency. A day counts as trained if
/// `vigorous >= T2` or `(moderate + eqDistance) >= T1`, where
/// `T1 = (M1 - 1.5) * 45`, `T2 = (M2 - 1.5) * 10` and `M1/M2` come from
/// `actinfo_get_default_met_values_for_categories(gender, categories)`
/// [PROVEN call; passed in here to keep the function pure]. Same ×1/7 ratio
/// and onboarding rule as c0. B @ 0x6f200 = [0, 0.5, 2.8, 4.0].
pub fn c3_training_frequency(
    history: &[ActivityDayRecord; 6],
    today: &ActivityDayRecord,
    m1: f64,
    m2: f64,
) -> f64 {
    let t1 = (m1 - 1.5) * 45.0;
    let t2 = (m2 - 1.5) * 10.0;
    let mut hits = 0u32;
    let mut days = 0u32;
    for d in history.iter().chain(std::iter::once(today)) {
        if !has_data(d) {
            continue;
        }
        days += 1;
        let trained =
            d.vigorous_met_minutes as f64 >= t2 || (d.moderate_met_minutes as f64 + d.eq_distance as f64) >= t1;
        if trained {
            hits += 1;
        }
    }
    if hits == 0 && days <= 1 {
        hits = 1;
        days = days.max(1);
    }
    let r = 7.0 * hits as f64 / days as f64;
    band_map(r, [0.0, 0.5, 2.8, 4.0], [0.5, 2.8, 4.0])
}

/// c4 — Training Volume. Recency-weighted weekly load: per day
/// `w[k] * (moderate + vigorous)` with day weights (rodata @ 0x7bc28)
/// `[19, 42, 77, 109, 134, 153, 166]` (sum 700; oldest slot = 19, today =
/// 166), summed, ÷ 100, ÷ (days_with_data × 1/7, clamped). B @ 0x6ece0 =
/// [0, 150, 850, 2000].
pub fn c4_training_volume(history: &[ActivityDayRecord; 6], today: &ActivityDayRecord) -> f64 {
    const W: [f64; 7] = [19.0, 42.0, 77.0, 109.0, 134.0, 153.0, 166.0];
    let mut weighted = 0.0;
    let mut days = 0u32;
    for (k, d) in history.iter().chain(std::iter::once(today)).enumerate() {
        if has_data(d) {
            days += 1;
        }
        weighted += W[k] * (d.moderate_met_minutes + d.vigorous_met_minutes) as f64;
    }
    // days_with_data × 1/7, clamped to ≥ 1/7 (÷0 guard; with no data days the
    // weighted sum is 0 anyway).
    let scale = (days as f64 / 7.0).max(1.0 / 7.0);
    let value = weighted / 100.0 / scale;
    band_map(value, [0.0, 150.0, 850.0, 2000.0], [150.0, 850.0, 2000.0])
}

/// c5 — "Recovery Time" (name INFERRED; numbers PROVEN). Feature-gated
/// (ecore feature 3 / 0x17) count `n` of restful days over the 7-day window:
/// feature path counts days with `field_14 > 45`; non-feature path counts
/// days with `field_12 < 15 && field_12 + field_14 < 85`. The third
/// MET-threshold variant (constants 1.01 / 4000.0) is not ported — not fully
/// recovered. Score: n == 0 → 0, n == 1 → 95, n ≥ 2 → 100
/// (B @ 0x6f210 = [0, 0.5, 1.0, 2.0]; n == 0 in ecore hits a NaN-compare
/// path that stores 0 — the band map yields 0 there too).
pub fn c5_recovery_time(
    history: &[ActivityDayRecord; 6],
    today: &ActivityDayRecord,
    feature_enabled: bool,
) -> f64 {
    let mut n = 0u32;
    for d in history.iter().chain(std::iter::once(today)) {
        let restful = if feature_enabled {
            d.field_14 > 45
        } else {
            d.field_12 < 15 && (d.field_12 as u32 + d.field_14 as u32) < 85
        };
        if restful {
            n += 1;
        }
    }
    band_map(n as f64, [0.0, 0.5, 1.0, 2.0], [0.5, 1.0, 2.0])
}

/// c6 — "Inactivity streak" (name INFERRED; numbers PROVEN). Walks the full
/// 9-slot history most-recent → oldest (0xc0 down to 0x00, step −0x18),
/// counting `1 + consecutive` inactive days (feature-0x17-off condition:
/// `field_12 + field_14 > 84 || field_14 > 14`; feature path: `field_14 >
/// 45`). Count capped at 10. B @ 0x6e8d0+0x10 = [9, 7, 3, 2]: ≥9 → 0,
/// 8 → 12.5, 7 → 25, 6 → 42.5, 5 → 60, 4 → 77.5, ≤3 → 95–100.
pub fn c6_inactivity_streak(state: &ActivityScoreState, feature_enabled: bool) -> f64 {
    let inactive = |d: &ActivityDayRecord| {
        if feature_enabled {
            d.field_14 > 45
        } else {
            (d.field_12 as u32 + d.field_14 as u32) > 84 || d.field_14 > 14
        }
    };
    let mut count = 1u32;
    for d in state.days.iter().rev() {
        if inactive(d) {
            count += 1;
        } else {
            break;
        }
    }
    let count = count.min(10);
    band_map(count as f64, [9.0, 7.0, 3.0, 2.0], [7.0, 3.0, 2.0])
}

/// Output of the combiner `get_activity_score_from_raw_100`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityScoreOutput {
    /// Final 0–100 activity score.
    pub score: u16,
    /// The seven contributors, rounded half up, clamped 0–100.
    pub contributors: [u16; 7],
    /// `out+0xc`: feature-gated secondary output.
    pub secondary: u16,
}

/// Round half up (ecore rounds with `+0.5` before truncation).
fn round_half_up(x: f64) -> u16 {
    (x + 0.5).floor().max(0.0) as u16
}

/// Combiner `get_activity_score_from_raw_100`: weights
/// `[25, 15, 10, 10, 15, 10, 15]` (sum 100) over the 0–100 contributors,
/// rounded half up; the feature-gated secondary output at `out+0xc` is
/// `(10*c5 + 15*c6)/25` when the feature is on, else `(c5 + c6)/2`.
pub fn activity_score_from_raw_100(contribs: [f64; 7], feature_enabled: bool) -> ActivityScoreOutput {
    const W: [f64; 7] = [25.0, 15.0, 10.0, 10.0, 15.0, 10.0, 15.0];
    let mut contributors = [0u16; 7];
    let mut weighted = 0.0;
    for i in 0..7 {
        let c = contribs[i].clamp(0.0, 100.0);
        contributors[i] = round_half_up(c);
        weighted += W[i] * c;
    }
    let score = round_half_up(weighted / 100.0).min(100);
    let secondary_raw = if feature_enabled {
        (10.0 * contribs[5].clamp(0.0, 100.0) + 15.0 * contribs[6].clamp(0.0, 100.0)) / 25.0
    } else {
        (contribs[5].clamp(0.0, 100.0) + contribs[6].clamp(0.0, 100.0)) / 2.0
    };
    ActivityScoreOutput {
        score,
        contributors,
        secondary: round_half_up(secondary_raw).min(100),
    }
}

/// All seven contributors from the state. `m1`/`m2` are the default MET
/// values for today's gender/category mask (c3); `feature_enabled` gates the
/// c5/c6 feature paths.
pub fn activity_contributors(
    state: &ActivityScoreState,
    m1: f64,
    m2: f64,
    feature_enabled: bool,
) -> [f64; 7] {
    let w7 = state.window7();
    let today = w7[6];
    let history: [ActivityDayRecord; 6] = w7[..6].try_into().unwrap();
    [
        c0_meet_daily_targets(&history, &today),
        c1_inactivity_time(&today),
        c2_movement_every_hour(&today),
        c3_training_frequency(&history, &today, m1, m2),
        c4_training_volume(&history, &today),
        c5_recovery_time(&history, &today, feature_enabled),
        c6_inactivity_streak(state, feature_enabled),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: f64, b: f64) {
        assert!(
            (a - b).abs() < 1e-9,
            "expected {b} (got {a}, diff {})",
            (a - b).abs()
        );
    }

    fn day() -> ActivityDayRecord {
        // Carries data (field sum != 0) but scores nothing by itself.
        // field_12 = 20 keeps it "not restful" for c5 and "active" for c6.
        ActivityDayRecord {
            sedentary_minutes: 1,
            field_12: 20,
            ..ActivityDayRecord::default()
        }
    }

    fn state_of(days: [ActivityDayRecord; 9]) -> ActivityScoreState {
        ActivityScoreState { days }
    }

    const C1_B: [f64; 4] = [1020.0, 955.0, 400.0, 300.0];
    const C1_U: [f64; 3] = [955.0, 400.0, 300.0];

    #[test]
    fn band_map_ascending_edges_and_clamps() {
        // c0-style table [0, 1, 5, 6] / uppers [1, 5, 6].
        let b = [0.0, 1.0, 5.0, 6.0];
        let u = [1.0, 5.0, 6.0];
        assert_eq!(band_map(0.0, b, u), 0.0);
        assert_eq!(band_map(1.0, b, u), 25.0);
        assert_eq!(band_map(0.5, b, u), 12.5); // 25 * 0.5 / 1
        assert_eq!(band_map(5.0, b, u), 95.0); // 25 + 70 * 4 / 4
        assert_eq!(band_map(3.0, b, u), 60.0); // 25 + 70 * 2 / 4
        assert_eq!(band_map(6.0, b, u), 100.0);
        assert_eq!(band_map(7.0, b, u), 100.0); // beyond outer bound → clamped
    }

    #[test]
    fn band_map_descending_edges_and_clamps() {
        // c1-style table: lower is better.
        assert_eq!(band_map(1020.0, C1_B, C1_U), 0.0);
        assert_eq!(band_map(987.5, C1_B, C1_U), 12.5); // 25 * 32.5 / 65
        assert_eq!(band_map(955.0, C1_B, C1_U), 25.0);
        assert_eq!(band_map(660.0, C1_B, C1_U), 25.0 + 4130.0 / 111.0);
        assert_eq!(band_map(400.0, C1_B, C1_U), 95.0);
        assert_eq!(band_map(300.0, C1_B, C1_U), 100.0);
        assert_eq!(band_map(100.0, C1_B, C1_U), 100.0); // ≤300 capped
        assert_eq!(band_map(2000.0, C1_B, C1_U), 0.0); // >1020 → 0
    }

    #[test]
    fn c0_hit_ratio_bands() {
        // 7 days with data → r = hits exactly.
        let mk = |status: u8| ActivityDayRecord {
            eq_distance: 100,
            eq_distance_target: 200,
            target_status: status,
            ..day()
        };
        let hist_hits = |hits: usize| -> [ActivityDayRecord; 6] {
            std::array::from_fn(|i| mk(if i < hits { 1 } else { 2 }))
        };
        let today_miss = mk(2);
        // r = 0 → 0
        assert_eq!(
            c0_meet_daily_targets(&hist_hits(0), &today_miss),
            0.0
        );
        // r = 1 → 25; r = 3 → 60; r = 5 → 95; r = 6 → 100; r = 7 → 100 (capped)
        assert_eq!(c0_meet_daily_targets(&hist_hits(1), &today_miss), 25.0);
        assert_eq!(c0_meet_daily_targets(&hist_hits(3), &today_miss), 60.0);
        assert_eq!(c0_meet_daily_targets(&hist_hits(5), &today_miss), 95.0);
        assert_eq!(c0_meet_daily_targets(&hist_hits(6), &today_miss), 100.0);
        assert_eq!(c0_meet_daily_targets(&hist_hits(5), &mk(1)), 100.0);
        // status 0 (unknown) compares distance: eq >= target counts as a hit.
        let unknown_hit = ActivityDayRecord {
            eq_distance: 300,
            eq_distance_target: 200,
            target_status: 0,
            ..day()
        };
        let unknown_miss = ActivityDayRecord {
            eq_distance: 100,
            eq_distance_target: 200,
            target_status: 0,
            ..day()
        };
        let hist = [unknown_hit; 6];
        assert_eq!(c0_meet_daily_targets(&hist, &unknown_miss), 100.0);
    }

    #[test]
    fn c0_onboarding_rule_counts_todays_miss_as_hit() {
        // No history data at all, today misses → treated as 1 hit of 1 day
        // → r = 7 → 100.
        let empty = [ActivityDayRecord::default(); 6];
        let today_miss = ActivityDayRecord {
            eq_distance: 100,
            eq_distance_target: 200,
            target_status: 2,
            ..day()
        };
        assert_eq!(c0_meet_daily_targets(&empty, &today_miss), 100.0);
    }

    #[test]
    fn c1_sedentary_breakpoints() {
        let mk = |mins: u16| ActivityDayRecord {
            sedentary_minutes: mins,
            ..day()
        };
        let today = mk(0);
        assert_eq!(c1_inactivity_time(&mk(1020)), 0.0);
        assert_eq!(c1_inactivity_time(&mk(955)), 25.0);
        assert_eq!(c1_inactivity_time(&mk(400)), 95.0);
        assert_eq!(c1_inactivity_time(&mk(300)), 100.0);
        assert_eq!(c1_inactivity_time(&mk(2000)), 0.0); // >1020 → 0
        assert_eq!(c1_inactivity_time(&today), 100.0); // ≤300 capped
        assert_close(
            c1_inactivity_time(&mk(660)),
            25.0 + 70.0 * (955.0 - 660.0) / 555.0,
        );
    }

    #[test]
    fn c2_sit_alert_breakpoints() {
        let mk = |alerts: u8| ActivityDayRecord {
            sit_alerts: alerts,
            ..day()
        };
        assert_eq!(c2_movement_every_hour(&mk(0)), 100.0);
        assert_eq!(c2_movement_every_hour(&mk(1)), 95.0);
        assert_eq!(c2_movement_every_hour(&mk(5)), 25.0);
        assert_eq!(c2_movement_every_hour(&mk(9)), 5.0); // 25 * (10-9) / 5
        assert_eq!(c2_movement_every_hour(&mk(10)), 0.0);
        assert_eq!(c2_movement_every_hour(&mk(255)), 0.0); // >9 → 0
    }

    #[test]
    fn c3_training_frequency_bands() {
        // m1 = m2 = 3.0 → T1 = 67.5, T2 = 15.
        let (m1, m2) = (3.0, 3.0);
        let mk = |vig: u16| ActivityDayRecord {
            vigorous_met_minutes: vig,
            ..day()
        };
        let hist = |hits: usize| -> [ActivityDayRecord; 6] {
            std::array::from_fn(|i| mk(if i < hits { 15 } else { 0 }))
        };
        let today_untrained = mk(0);
        // 7 days with data → r = hits: 0 → 0, 4 → 100 (r = 4 = outer bound).
        assert_eq!(c3_training_frequency(&hist(0), &today_untrained, m1, m2), 0.0);
        assert_eq!(c3_training_frequency(&hist(4), &today_untrained, m1, m2), 100.0);
        // 7 hits (6 history + today) → r = 7 → capped 100.
        let today_trained = mk(15);
        assert_eq!(c3_training_frequency(&hist(6), &today_trained, m1, m2), 100.0);
        // r = 3 → band 3: 95 + 5 * (3 - 2.8) / 1.2 ≈ 95.8333…
        assert_close(
            c3_training_frequency(&hist(3), &today_untrained, m1, m2),
            95.0 + 5.0 / 6.0,
        );
        // r = 1 → band 2: 25 + 70 * 0.5 / 2.3 = 25 + 350/23.
        assert_close(
            c3_training_frequency(&hist(1), &today_untrained, m1, m2),
            25.0 + 350.0 / 23.0,
        );
        // Onboarding: only today, untrained → r = 7 → 100.
        let empty = [ActivityDayRecord::default(); 6];
        assert_eq!(
            c3_training_frequency(&empty, &today_untrained, m1, m2),
            100.0
        );
        // Partial window: 1 trained day, and today carries no data →
        // 1 day with data, 1 hit → r = 7 → 100.
        let mut one_day = [ActivityDayRecord::default(); 6];
        one_day[0] = mk(15);
        let no_data_today = ActivityDayRecord::default();
        assert_eq!(
            c3_training_frequency(&one_day, &no_data_today, m1, m2),
            100.0
        );
    }

    #[test]
    fn c4_weight_table_dot_product() {
        let mk = |load: u16| ActivityDayRecord {
            moderate_met_minutes: load,
            ..day()
        };
        // All 7 days (mod+vig) = 15 → Σ w·load = 15·700 = 10500 → /100 = 105
        // → 7 days → ÷1 → band 1: 25·105/150 = 17.5.
        let hist: [ActivityDayRecord; 6] = std::array::from_fn(|_| mk(15));
        assert_eq!(c4_training_volume(&hist, &mk(15)), 17.5);
        // Every day load 1 → value 7 → 25·7/150 = 7/6.
        let hist: [ActivityDayRecord; 6] = std::array::from_fn(|_| mk(1));
        assert_close(c4_training_volume(&hist, &mk(1)), 7.0 / 6.0);
        // Weight order with a full 7-day window (every day carries data via
        // its sedentary field, so the divisor is 1): only the oldest slot
        // loaded → 19·100/100 = 19 → 25·19/150 = 19/6; only today loaded →
        // 166·100/100 = 166 → 25 + 70·16/700 = 26.6.
        let mut hist: [ActivityDayRecord; 6] = std::array::from_fn(|_| mk(0));
        hist[0] = mk(100);
        assert_close(c4_training_volume(&hist, &mk(0)), 19.0 / 6.0);
        let hist: [ActivityDayRecord; 6] = std::array::from_fn(|_| mk(0));
        assert_close(c4_training_volume(&hist, &mk(100)), 26.6);
        // Missing-days scaling: only today carries data → 1 day → ÷(1/7):
        // 166·100/100 = 166 → ×7 = 1162 → band 3: 95 + 5·312/1150
        // = 95 + 156/115.
        let hist = [ActivityDayRecord::default(); 6];
        assert_close(c4_training_volume(&hist, &mk(100)), 95.0 + 156.0 / 115.0);
        // Above the outer bound: all days 300 → 2100 → capped 100.
        let hist: [ActivityDayRecord; 6] = std::array::from_fn(|_| mk(300));
        assert_eq!(c4_training_volume(&hist, &mk(300)), 100.0);
        // Band 3 interior: all days 250 → 1750 → 95 + 5·900/1150 = 95 + 90/23.
        let hist: [ActivityDayRecord; 6] = std::array::from_fn(|_| mk(250));
        assert_close(c4_training_volume(&hist, &mk(250)), 95.0 + 90.0 / 23.0);
    }

    #[test]
    fn c5_rest_day_count() {
        // Feature path: field_14 > 45.
        let restful_f = ActivityDayRecord {
            field_14: 50,
            ..day()
        };
        let busy = day(); // field_14 = 0 → not restful in either path
        let hist = [busy, busy, busy, busy, busy, restful_f];
        assert_eq!(c5_recovery_time(&hist, &busy, true), 95.0); // n = 1
        assert_eq!(c5_recovery_time(&hist, &restful_f, true), 100.0); // n = 2
        // n = 0 → 0 (ecore's NaN-compare path also stores 0).
        let all_busy = [busy; 6];
        assert_eq!(c5_recovery_time(&all_busy, &busy, true), 0.0);
        // Non-feature path: field_12 < 15 && field_12 + field_14 < 85.
        let restful_n = ActivityDayRecord {
            field_12: 10,
            field_14: 20,
            ..day()
        };
        let not_restful = ActivityDayRecord {
            field_12: 20,
            ..day()
        }; // field_12 ≥ 15
        let boundary = ActivityDayRecord {
            field_12: 0,
            field_14: 85,
            ..day()
        }; // sum = 85, not < 85
        let hist = [not_restful, boundary, not_restful, not_restful, not_restful, restful_n];
        assert_eq!(c5_recovery_time(&hist, &busy, false), 95.0);
        assert_eq!(c5_recovery_time(&hist, &restful_n, false), 100.0);
    }

    #[test]
    fn c6_inactivity_streak_counts_1_plus_consecutive() {
        // Feature-off condition: f12+f14 > 84 || f14 > 14.
        let inactive = ActivityDayRecord {
            field_14: 85,
            ..day()
        };
        let active = ActivityDayRecord {
            field_12: 20,
            field_14: 10,
            ..day()
        };
        let mk_state = |trailing_inactive: usize| -> ActivityScoreState {
            state_of(std::array::from_fn(|i| {
                if i >= 9 - trailing_inactive.min(9) {
                    inactive
                } else {
                    active
                }
            }))
        };
        // No inactive days → count 1 → 100 (extrapolated clamp above 95–100 band).
        assert_eq!(c6_inactivity_streak(&mk_state(0), false), 100.0);
        // 1 trailing inactive → count 2 → 100; 2 → count 3 → 95.
        assert_eq!(c6_inactivity_streak(&mk_state(1), false), 100.0);
        assert_eq!(c6_inactivity_streak(&mk_state(2), false), 95.0);
        // 4 trailing → count 5 → 60 = 25 + 70·(7−5)/4.
        assert_eq!(c6_inactivity_streak(&mk_state(4), false), 60.0);
        // 5 → 42.5; 6 → 25; 7 → 12.5.
        assert_eq!(c6_inactivity_streak(&mk_state(5), false), 42.5);
        assert_eq!(c6_inactivity_streak(&mk_state(6), false), 25.0);
        assert_eq!(c6_inactivity_streak(&mk_state(7), false), 12.5);
        // 9 trailing → count 10 (capped) → 0.
        assert_eq!(c6_inactivity_streak(&mk_state(9), false), 0.0);
        // Feature path: f14 > 45.
        let inactive_f = ActivityDayRecord {
            field_14: 50,
            ..day()
        };
        let st = state_of([
            active, active, active, active, inactive_f, inactive_f, inactive_f, inactive_f,
            inactive_f,
        ]);
        assert_eq!(c6_inactivity_streak(&st, true), 42.5);
    }

    #[test]
    fn combiner_weights_and_rounding() {
        // All contributors 100 → score 100.
        let out = activity_score_from_raw_100([100.0; 7], true);
        assert_eq!(out.score, 100);
        assert_eq!(out.contributors, [100; 7]);
        assert_eq!(out.secondary, 100);
        // Weight proof: only c0 = 100 → 25·100/100 = 25.
        assert_eq!(activity_score_from_raw_100([100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], true).score, 25);
        assert_eq!(activity_score_from_raw_100([0.0, 100.0, 0.0, 0.0, 0.0, 0.0, 0.0], true).score, 15);
        assert_eq!(activity_score_from_raw_100([0.0, 0.0, 100.0, 0.0, 0.0, 0.0, 0.0], true).score, 10);
        assert_eq!(activity_score_from_raw_100([0.0, 0.0, 0.0, 100.0, 0.0, 0.0, 0.0], true).score, 10);
        assert_eq!(activity_score_from_raw_100([0.0, 0.0, 0.0, 0.0, 100.0, 0.0, 0.0], true).score, 15);
        assert_eq!(activity_score_from_raw_100([0.0, 0.0, 0.0, 0.0, 0.0, 100.0, 0.0], true).score, 10);
        assert_eq!(activity_score_from_raw_100([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 100.0], true).score, 15);
    }

    #[test]
    fn combiner_secondary_feature_branch() {
        let c = [0.0, 0.0, 0.0, 0.0, 0.0, 100.0, 0.0];
        // Feature on: (10·100 + 15·0)/25 = 40. Feature off: 100/2 = 50.
        assert_eq!(activity_score_from_raw_100(c, true).secondary, 40);
        assert_eq!(activity_score_from_raw_100(c, false).secondary, 50);
        let c = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 100.0];
        // Feature on: 15·100/25 = 60. Feature off: 50. The 40/60 pair proves
        // the ÷25 divisor; 50/50 would be indistinguishable from an average.
        assert_eq!(activity_score_from_raw_100(c, true).secondary, 60);
        assert_eq!(activity_score_from_raw_100(c, false).secondary, 50);
    }

    #[test]
    fn combiner_half_up_rounding_boundary() {
        // 25·2/100 = 0.5 → rounds half UP to 1 (not banker's rounding).
        assert_eq!(activity_score_from_raw_100([2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], true).score, 1);
        // 25·1/100 = 0.25 → 0.
        assert_eq!(activity_score_from_raw_100([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], true).score, 0);
        // Secondary: (1+0)/2 = 0.5 → 1; feature path 10·1/25 = 0.4 → 0.
        let c = [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        assert_eq!(activity_score_from_raw_100(c, false).secondary, 1);
        assert_eq!(activity_score_from_raw_100(c, true).secondary, 0);
        // Contributors round half up too: 42.5 → 43, 42.4 → 42.
        let c = [42.5, 42.4, 0.0, 0.0, 0.0, 0.0, 0.0];
        let out = activity_score_from_raw_100(c, false);
        assert_eq!(out.contributors[0], 43);
        assert_eq!(out.contributors[1], 42);
    }

    #[test]
    fn combiner_clamps_contributors() {
        // >100 clamps to 100 for both the stored contributor and the score.
        let out = activity_score_from_raw_100([150.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], true);
        assert_eq!(out.contributors[0], 100);
        assert_eq!(out.score, 25);
        // <0 clamps to 0.
        let out = activity_score_from_raw_100([-10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], true);
        assert_eq!(out.contributors[0], 0);
        assert_eq!(out.score, 0);
        // Secondary clamps at 100.
        let out = activity_score_from_raw_100([0.0, 0.0, 0.0, 0.0, 0.0, 100.0, 100.0], true);
        assert_eq!(out.secondary, 100);
    }

    #[test]
    fn end_to_end_all_contributors() {
        // Empty state (no day carries data): c0/c3 onboarding → 100,
        // c1/c2 rest at min → 100, c4 → 0, c5 (feature on) → 0, c6 → 100.
        let st = state_of([ActivityDayRecord::default(); 9]);
        assert_eq!(
            activity_contributors(&st, 3.0, 3.0, true),
            [100.0, 100.0, 100.0, 100.0, 0.0, 0.0, 100.0]
        );
        // Combiner: (25+15+10+10+0+0+15) = 75.
        let contribs = activity_contributors(&st, 3.0, 3.0, true);
        let out = activity_score_from_raw_100(contribs, true);
        assert_eq!(out.score, 75);
    }
}

