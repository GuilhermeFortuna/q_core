//! NumPy-compatible float64 pairwise summation (`np.sum` / `add.reduce` order).

const PW_BLOCKSIZE: usize = 128;

/// Sums `values` in NumPy float64 `add.reduce` pairwise order.
pub(crate) fn numpy_sum_f64(values: &[f64]) -> f64 {
    pairwise_sum(values)
}

fn pairwise_sum(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 8 {
        let mut res = 0.0;
        for &v in values {
            res += v;
        }
        return res;
    }
    if n <= PW_BLOCKSIZE {
        let mut r = [0.0_f64; 8];
        for (acc, &v) in r.iter_mut().zip(values.iter().take(8)) {
            *acc = v;
        }
        let mut i = 8;
        while i + 8 <= n {
            r[0] += values[i];
            r[1] += values[i + 1];
            r[2] += values[i + 2];
            r[3] += values[i + 3];
            r[4] += values[i + 4];
            r[5] += values[i + 5];
            r[6] += values[i + 6];
            r[7] += values[i + 7];
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for &v in &values[i..] {
            res += v;
        }
        return res;
    }
    let mut n2 = n / 2;
    n2 -= n2 % 8;
    pairwise_sum(&values[..n2]) + pairwise_sum(&values[n2..])
}

#[cfg(test)]
mod tests {
    use super::numpy_sum_f64;

    const PCG64_MULT: u128 = (2549297995355413924_u128 << 64) | 4865540595714422341_u128;
    const PCG64_INIT_STATE: u128 = 35399562948360463058890781895381311971;
    const PCG64_INIT_INC: u128 = 87136372517582989555478159403783844777;

    struct NumpyDefaultRng {
        state: u128,
        inc: u128,
    }

    impl NumpyDefaultRng {
        fn new() -> Self {
            Self {
                state: PCG64_INIT_STATE,
                inc: PCG64_INIT_INC,
            }
        }

        fn random_f64(&mut self) -> f64 {
            self.state = self.state.wrapping_mul(PCG64_MULT).wrapping_add(self.inc);
            let hi = (self.state >> 64) as u64;
            let lo = self.state as u64;
            let xored = hi ^ lo;
            let rot = (hi >> 58) as u32;
            let raw = xored.rotate_right(rot);
            (raw >> 11) as f64 * (1.0 / 9007199254740992.0)
        }

        fn random(&mut self, n: usize) -> Vec<f64> {
            (0..n).map(|_| self.random_f64()).collect()
        }
    }

    fn assert_sum_bits(values: &[f64], expected_bits: u64) {
        let sum = numpy_sum_f64(values);
        assert_eq!(
            sum.to_bits(),
            expected_bits,
            "sum={sum} expected={}",
            f64::from_bits(expected_bits)
        );
    }

    #[test]
    fn numpy_sum_matches_recorded_bits_for_seeded_lengths() {
        let mut rng = NumpyDefaultRng::new();
        let cases: &[(usize, u64)] = &[
            (1, 0x3fe461fd79fb3850),
            (7, 0x400b1d95e0d98bcb),
            (8, 0x40105fd628a6cd26),
            (9, 0x4010d9f30dcf7d95),
            (127, 0x40512c7cb3d4e8b2),
            (128, 0x40518af4af760746),
            (129, 0x40503972a611f7c8),
            (1000, 0x407f2e843e496f4b),
            (20000, 0x40c39e215c1fb7c1),
        ];
        for &(len, expected_bits) in cases {
            let values = rng.random(len);
            assert_sum_bits(&values, expected_bits);
        }
    }

    #[test]
    fn numpy_sum_neg_zero_has_numpy_sign_bit() {
        let sum = numpy_sum_f64(&[-0.0]);
        assert_eq!(sum.to_bits(), 0x0000_0000_0000_0000);
        assert!(!sum.is_sign_negative());
    }
}
