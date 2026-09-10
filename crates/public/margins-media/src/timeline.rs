//! Platform-neutral media-frame conversion and streaming resampling.

use anyhow::{bail, Context, Result};

/// Convert elapsed nanoseconds to sample frames using checked, floor-rounded
/// integer math.
pub fn target_frame(duration_nanos: u64, rate: u32) -> Result<u64> {
    let frames = u128::from(duration_nanos)
        .checked_mul(u128::from(rate))
        .context("media-frame multiplication overflow")?
        / 1_000_000_000u128;
    u64::try_from(frames).context("media-frame count exceeds u64")
}

/// Scale a frame count between sample rates using checked, floor-rounded
/// integer math.
pub fn scale_frames(frames: u64, from_rate: u32, to_rate: u32) -> Result<u64> {
    if from_rate == 0 || to_rate == 0 {
        bail!("sample rates must be non-zero");
    }
    let scaled = u128::from(frames)
        .checked_mul(u128::from(to_rate))
        .context("frame-rate conversion overflow")?
        / u128::from(from_rate);
    u64::try_from(scaled).context("converted frame count exceeds u64")
}

/// Linear, streaming rational resampler.
///
/// Output positions are represented by integer cross-products, so chunk
/// boundaries never reset phase. [`Self::finish`] pads the final interpolation
/// interval with the last input sample so total output is exactly
/// `floor(total_input * to_rate / from_rate)`.
#[derive(Debug)]
pub struct RationalResampler {
    from_rate: u32,
    to_rate: u32,
    input_count: u64,
    output_count: u64,
    previous: Option<f32>,
    finished: bool,
}

impl RationalResampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Result<Self> {
        if from_rate == 0 || to_rate == 0 {
            bail!("resampler rates must be non-zero");
        }
        Ok(Self {
            from_rate,
            to_rate,
            input_count: 0,
            output_count: 0,
            previous: None,
            finished: false,
        })
    }

    pub fn from_rate(&self) -> u32 {
        self.from_rate
    }

    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        if self.finished {
            bail!("resampler used after finish");
        }
        let estimate = scale_frames(input.len() as u64 + 2, self.from_rate, self.to_rate)?;
        let mut output = Vec::with_capacity(usize::try_from(estimate).unwrap_or(0));
        for &current in input {
            let index = self.input_count;
            self.input_count += 1;
            let target = scale_frames(self.input_count, self.from_rate, self.to_rate)?;
            let previous = self.previous.unwrap_or(current);
            while self.output_count < target {
                // Output j represents source position
                // ((j + 1) * from / to) - 1. This end-aligned mapping gives
                // exactly floor(N * to / from) outputs after N inputs.
                let unshifted = u128::from(self.output_count + 1) * u128::from(self.from_rate);
                let shift = u128::from(self.to_rate);
                let sample = if index == 0 || unshifted <= shift {
                    current
                } else {
                    let position = unshifted - shift;
                    let lower = u128::from(index - 1) * u128::from(self.to_rate);
                    let numerator = position.saturating_sub(lower).min(shift);
                    let fraction = numerator as f32 / self.to_rate as f32;
                    previous + (current - previous) * fraction
                };
                output.push(sample);
                self.output_count += 1;
            }
            self.previous = Some(current);
        }
        Ok(output)
    }

    pub fn finish(&mut self) -> Result<Vec<f32>> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        let target = scale_frames(self.input_count, self.from_rate, self.to_rate)?;
        let mut output = Vec::with_capacity(
            usize::try_from(target.saturating_sub(self.output_count)).unwrap_or(0),
        );
        let last = self.previous.unwrap_or(0.0);
        while self.output_count < target {
            output.push(last);
            self.output_count += 1;
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_frame_uses_floor_integer_math_at_boundaries() {
        assert_eq!(target_frame(999_999_999, 48_000).unwrap(), 47_999);
        assert_eq!(target_frame(1_000_000_000, 48_000).unwrap(), 48_000);
        assert_eq!(target_frame(1, 44_100).unwrap(), 0);
    }

    #[test]
    fn frame_scaling_is_checked_and_floor_rounded() {
        assert_eq!(scale_frames(44_100, 44_100, 48_000).unwrap(), 48_000);
        assert_eq!(scale_frames(1, 48_000, 44_100).unwrap(), 0);
        assert!(scale_frames(1, 0, 48_000).is_err());
    }

    #[test]
    fn rational_resampler_is_chunk_boundary_independent() {
        let input: Vec<f32> = (0..10_003).map(|n| (n % 31) as f32 / 31.0).collect();
        let mut one = RationalResampler::new(44_100, 48_000).unwrap();
        let mut expected = one.process(&input).unwrap();
        expected.extend(one.finish().unwrap());

        let mut chunked = RationalResampler::new(44_100, 48_000).unwrap();
        let mut actual = Vec::new();
        for chunk in input.chunks(137) {
            actual.extend(chunked.process(chunk).unwrap());
        }
        actual.extend(chunked.finish().unwrap());
        assert_eq!(actual, expected);
    }

    #[test]
    fn rational_resampler_441_to_48_is_long_run_drift_free() {
        let mut resampler = RationalResampler::new(44_100, 48_000).unwrap();
        let input = vec![0.25; 44_100 * 20];
        let mut count = 0usize;
        for chunk in input.chunks(997) {
            count += resampler.process(chunk).unwrap().len();
        }
        count += resampler.finish().unwrap().len();
        assert_eq!(count, 48_000 * 20);
    }

    #[test]
    fn rational_resampler_matches_one_shot_across_rates_lengths_and_boundaries() {
        let rate_pairs = [
            (1, 1),
            (2, 3),
            (3, 2),
            (8_000, 16_000),
            (16_000, 8_000),
            (44_100, 48_000),
            (48_000, 44_100),
        ];
        let lengths = [0, 1, 2, 3, 7, 31, 257];
        let chunk_sizes = [1, 2, 3, 5, 17, 64, 251];

        for (from_rate, to_rate) in rate_pairs {
            for length in lengths {
                let input: Vec<f32> = (0..length)
                    .map(|index| ((index * 17 + 3) % 29) as f32 / 29.0)
                    .collect();
                let mut one_shot = RationalResampler::new(from_rate, to_rate).unwrap();
                let mut expected = one_shot.process(&input).unwrap();
                expected.extend(one_shot.finish().unwrap());
                assert_eq!(
                    expected.len() as u64,
                    scale_frames(length as u64, from_rate, to_rate).unwrap()
                );

                for chunk_size in chunk_sizes {
                    let mut chunked = RationalResampler::new(from_rate, to_rate).unwrap();
                    let mut actual = Vec::new();
                    for chunk in input.chunks(chunk_size) {
                        actual.extend(chunked.process(chunk).unwrap());
                    }
                    actual.extend(chunked.finish().unwrap());
                    assert_eq!(
                        actual, expected,
                        "from={from_rate}, to={to_rate}, len={length}, chunk={chunk_size}"
                    );
                }
            }
        }
    }

    #[test]
    fn frame_math_rejects_unrepresentable_results() {
        assert!(target_frame(u64::MAX, u32::MAX).is_err());
        assert!(scale_frames(u64::MAX, 1, u32::MAX).is_err());
    }
}
