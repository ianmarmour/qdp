//! Summaries of measured samples.

/// Returns the `percent` percentile of `samples`, by the nearest rank, or
/// `None` if there are none.
pub fn percentile(samples: &[f64], percent: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (percent / 100.0 * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// Returns the mean of `samples`, or `None` if there are none.
pub fn mean(samples: &[f64]) -> Option<f64> {
    (!samples.is_empty()).then(|| samples.iter().sum::<f64>() / samples.len() as f64)
}

/// Returns the standard deviation of `samples`, or `None` if there are
/// fewer than two.
pub fn standard_deviation(samples: &[f64]) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }
    let mean = mean(samples)?;
    let variance = samples
        .iter()
        .map(|sample| (sample - mean).powi(2))
        .sum::<f64>()
        / (samples.len() - 1) as f64;
    Some(variance.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Percentiles take the nearest rank.
    #[test]
    fn takes_the_nearest_rank() {
        let samples = [5.0, 1.0, 4.0, 2.0, 3.0];
        assert_eq!(percentile(&samples, 50.0), Some(3.0));
        assert_eq!(percentile(&samples, 0.0), Some(1.0));
        assert_eq!(percentile(&samples, 99.0), Some(5.0));
        assert_eq!(percentile(&[], 50.0), None);
    }

    /// The mean and standard deviation are the sample's.
    #[test]
    fn summarises_spread() {
        let samples = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        assert_eq!(mean(&samples), Some(5.0));
        let deviation = standard_deviation(&samples).unwrap();
        assert!((deviation - 2.138).abs() < 0.001, "{deviation}");
    }
}
