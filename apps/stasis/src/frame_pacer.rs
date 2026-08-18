use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub(crate) struct FramePacer {
    interval: Duration,
    next_deadline: Instant,
}

impl FramePacer {
    pub(crate) fn from_micros(interval_micros: u64, now: Instant) -> Result<Option<Self>, String> {
        if interval_micros == 0 {
            return Ok(None);
        }
        let interval = Duration::from_micros(interval_micros);
        let next_deadline = now
            .checked_add(interval)
            .ok_or_else(|| "tick interval exceeds the host monotonic clock range".to_string())?;
        Ok(Some(Self {
            interval,
            next_deadline,
        }))
    }

    pub(crate) fn wait(&mut self) {
        let delay = self.delay_at(Instant::now());
        if !delay.is_zero() {
            thread::sleep(delay);
        }
    }

    fn delay_at(&mut self, now: Instant) -> Duration {
        let deadline = self.next_deadline;
        if now < deadline {
            self.next_deadline = deadline.checked_add(self.interval).unwrap_or(now);
            return deadline.duration_since(now);
        }

        if now.duration_since(deadline) >= self.interval {
            self.next_deadline = now.checked_add(self.interval).unwrap_or(now);
        } else {
            self.next_deadline = deadline.checked_add(self.interval).unwrap_or(now);
        }
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pacer(interval_micros: u64, base: Instant) -> FramePacer {
        FramePacer::from_micros(interval_micros, base)
            .expect("valid interval")
            .expect("enabled pacer")
    }

    #[test]
    fn fast_present_waits_only_for_remaining_frame_budget() {
        let base = Instant::now();
        let mut pacer = pacer(16_000, base);

        assert_eq!(
            pacer.delay_at(base + Duration::from_micros(4_000)),
            Duration::from_micros(12_000)
        );
        assert_eq!(pacer.next_deadline, base + Duration::from_micros(32_000));
    }

    #[test]
    fn vsynced_present_consuming_the_interval_adds_no_delay() {
        let base = Instant::now();
        let mut pacer = pacer(16_000, base);

        assert_eq!(
            pacer.delay_at(base + Duration::from_micros(16_000)),
            Duration::ZERO
        );
        assert_eq!(pacer.next_deadline, base + Duration::from_micros(32_000));
    }

    #[test]
    fn short_overrun_does_not_add_delay_or_move_the_cadence() {
        let base = Instant::now();
        let mut pacer = pacer(16_000, base);

        assert_eq!(
            pacer.delay_at(base + Duration::from_micros(20_000)),
            Duration::ZERO
        );
        assert_eq!(pacer.next_deadline, base + Duration::from_micros(32_000));
    }

    #[test]
    fn long_pause_resets_without_a_catch_up_burst() {
        let base = Instant::now();
        let mut pacer = pacer(16_000, base);
        let resumed = base + Duration::from_secs(2);

        assert_eq!(pacer.delay_at(resumed), Duration::ZERO);
        assert_eq!(pacer.next_deadline, resumed + Duration::from_micros(16_000));
        assert_eq!(
            pacer.delay_at(resumed + Duration::from_micros(1_000)),
            Duration::from_micros(15_000)
        );
    }

    #[test]
    fn zero_interval_disables_pacing() {
        assert!(FramePacer::from_micros(0, Instant::now())
            .expect("zero is valid")
            .is_none());
    }
}
