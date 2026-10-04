const EXACT_BUCKETS: usize = 1_024;
const SUB_BUCKETS: usize = 64;
const BUCKETS: usize = 4_096;

pub struct Histogram {
    buckets: Box<[u64; BUCKETS]>,
    count: u64,
}

impl Histogram {
    pub fn new() -> Self {
        Self {
            buckets: Box::new([0; BUCKETS]),
            count: 0,
        }
    }

    pub fn record(&mut self, microseconds: u64) {
        self.buckets[index(microseconds)] += 1;
        self.count += 1;
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn percentile_ms(&self, percentile: u64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let target = self.count.saturating_mul(percentile).div_ceil(100);
        let mut observed = 0;
        for (index, count) in self.buckets.iter().enumerate() {
            observed += count;
            if observed >= target {
                return upper_bound(index) as f64 / 1_000.0;
            }
        }
        upper_bound(BUCKETS - 1) as f64 / 1_000.0
    }
}

fn index(microseconds: u64) -> usize {
    if microseconds < EXACT_BUCKETS as u64 {
        return microseconds as usize;
    }
    let exponent = 63 - microseconds.leading_zeros() as usize;
    let lower = 1_u64 << exponent;
    let step = (lower / SUB_BUCKETS as u64).max(1);
    let sub = ((microseconds - lower) / step).min((SUB_BUCKETS - 1) as u64) as usize;
    (EXACT_BUCKETS + (exponent - 10) * SUB_BUCKETS + sub).min(BUCKETS - 1)
}

fn upper_bound(index: usize) -> u64 {
    if index < EXACT_BUCKETS {
        return index as u64;
    }
    let offset = index - EXACT_BUCKETS;
    let exponent = 10 + offset / SUB_BUCKETS;
    if exponent >= 63 {
        return u64::MAX;
    }
    let sub = offset % SUB_BUCKETS;
    let lower = 1_u64 << exponent;
    let step = lower / SUB_BUCKETS as u64;
    lower.saturating_add(step.saturating_mul(sub as u64 + 1))
}
