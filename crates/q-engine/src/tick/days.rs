//! UTC day run bounds over tick timestamps (`floor(time_msc / 86_400_000)`).

const DAY_MS: i64 = 86_400_000;

/// `(starts, ends)` of consecutive runs of equal `floor(time_msc / 86_400_000)`.
///
/// Half-open index ranges `[starts[i], ends[i])` into `time_msc`.
pub fn tick_day_bounds(time_msc: &[i64]) -> (Vec<i64>, Vec<i64>) {
    let n = time_msc.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut starts = vec![0_i64];
    let mut ends = Vec::new();
    let mut prev_day = time_msc[0].div_euclid(DAY_MS);
    for (i, &t) in time_msc.iter().enumerate().skip(1) {
        let day = t.div_euclid(DAY_MS);
        if day != prev_day {
            ends.push(i as i64);
            starts.push(i as i64);
            prev_day = day;
        }
    }
    ends.push(n as i64);
    (starts, ends)
}

#[cfg(test)]
mod tests {
    use super::tick_day_bounds;

    #[test]
    fn two_utc_midnights_give_three_runs() {
        // 1970-01-01, 1970-01-02, 1970-01-03 (UTC midnights).
        let times = [0_i64, 86_400_000, 172_800_000];
        let (starts, ends) = tick_day_bounds(&times);
        assert_eq!(starts, vec![0, 1, 2]);
        assert_eq!(ends, vec![1, 2, 3]);
    }

    #[test]
    fn negative_one_ms_is_day_before_epoch() {
        let (starts, ends) = tick_day_bounds(&[-1]);
        assert_eq!(starts, vec![0]);
        assert_eq!(ends, vec![1]);
        // Day id is -1 (the calendar day before 1970-01-01).
        assert_eq!((-1_i64).div_euclid(86_400_000), -1);
    }

    #[test]
    fn empty_stream_has_no_days() {
        let (starts, ends) = tick_day_bounds(&[]);
        assert!(starts.is_empty());
        assert!(ends.is_empty());
    }
}
