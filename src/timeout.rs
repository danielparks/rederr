use std::cmp::Ordering;
use std::fmt;
use std::time::{Duration, Instant};

/// Minimum valid timeout that poll() respects.
const TIMEOUT_RESOLUTION: Duration = Duration::from_millis(1);

/// A stateful timeout.
///
/// Create a `Timeout::Future` to represent a planned timeout. Run `.start()`
/// to get a new `Timeout::Pending` that tracks how much time has passed, then
/// call `.check_expired()` on that to get `Timeout::Expired` when the timeout
/// has expired.
#[derive(Clone, Eq, Debug)]
pub enum Timeout {
    Never,
    Future {
        timeout: Duration,
    },
    Pending {
        timeout: Duration,
        start: Instant,
    },
    Expired {
        requested: Duration,
        actual: Duration,
    },
}

impl Timeout {
    /// Get the remaining timeout if available.
    ///
    /// Returns Some(Duration::ZERO) if the timeout has already expired.
    pub fn timeout(&self) -> Option<Duration> {
        match &self {
            Self::Never => None,
            Self::Future { timeout } => Some(*timeout),
            Self::Pending { timeout, start } => {
                Some(timeout.saturating_sub(start.elapsed()))
            }
            Self::Expired { .. } => Some(Duration::ZERO),
        }
    }

    /// Return a pending version of this `Timeout`.
    ///
    /// If the timeout is `Never`, `Pending`, or `Expired`, then it returns a
    /// clone of `self`.
    pub fn start(&self) -> Self {
        if let Self::Future { timeout } = self {
            Self::Pending {
                timeout: *timeout,
                start: Instant::now(),
            }
        } else {
            self.clone()
        }
    }

    /// Has the timeout expired?
    pub fn check_expired(&self) -> Option<Self> {
        match &self {
            Self::Pending { timeout, start } => {
                let elapsed = start.elapsed();
                if timeout.saturating_sub(elapsed) < TIMEOUT_RESOLUTION {
                    Some(Self::Expired {
                        requested: *timeout,
                        actual: elapsed,
                    })
                } else {
                    None
                }
            }
            // FIXME better way of doing this?
            Self::Expired { .. } => Some(self.clone()),
            _ => None,
        }
    }

    /// How much of the timeout has elapsed.
    pub fn elapsed(&self) -> Duration {
        match &self {
            Self::Never => Duration::ZERO,
            Self::Future { .. } => Duration::ZERO,
            Self::Pending { start, .. } => start.elapsed(),
            Self::Expired { actual, .. } => *actual,
        }
    }

    /// How much of the timeout has elapsed, rounded to the nearest ms.
    pub fn elapsed_rounded(&self) -> Duration {
        // FIXME: actually consult resolution?
        let elapsed = self.elapsed();
        let nanos = elapsed.subsec_nanos();
        let sub_ms = nanos % 1_000_000;

        let rounded = if sub_ms < 500_000 {
            nanos - sub_ms
        } else {
            nanos + 1_000_000 - sub_ms
        };

        Duration::new(elapsed.as_secs(), rounded)
    }
}

impl fmt::Display for Timeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            Self::Never => write!(f, "Never"),
            Self::Future { timeout } => {
                write!(f, "Future({timeout:?})")
            }
            Self::Pending { timeout, start } => {
                write!(
                    f,
                    "Pending({:?}, {:?} remaining)",
                    timeout,
                    timeout.saturating_sub(start.elapsed()),
                )
            }
            Self::Expired { requested, actual } => {
                write!(f, "Expired({requested:?} requested, {actual:?} actual)")
            }
        }
    }
}

impl From<Duration> for Timeout {
    fn from(timeout: Duration) -> Self {
        Self::Future { timeout }
    }
}

impl From<Option<Duration>> for Timeout {
    fn from(timeout: Option<Duration>) -> Self {
        match timeout {
            Some(timeout) => Self::from(timeout),
            None => Self::Never,
        }
    }
}

impl Ord for Timeout {
    fn cmp(&self, other: &Self) -> Ordering {
        // FIXME: should Expired always be shortest?
        match (self.timeout(), other.timeout()) {
            (None, None) => Ordering::Equal,
            (None, _) => Ordering::Greater,
            (_, None) => Ordering::Less,
            (Some(a), Some(b)) => a.cmp(&b),
        }
    }
}

impl PartialOrd for Timeout {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Timeout {
    fn eq(&self, other: &Self) -> bool {
        self.timeout().eq(&other.timeout())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;
    use std::time::Duration;

    fn future_timeout(microseconds: u64) -> Timeout {
        Timeout::Future {
            timeout: Duration::from_micros(microseconds),
        }
    }

    fn pending_timeout(microseconds: u64, elapsed: u64) -> Timeout {
        Timeout::Pending {
            timeout: Duration::from_micros(microseconds),
            start: Instant::now()
                .checked_sub(Duration::from_micros(elapsed))
                .unwrap(),
        }
    }

    fn expired_timeout(microseconds: u64) -> Timeout {
        Timeout::Expired {
            requested: Duration::from_micros(microseconds),
            actual: Duration::from_micros(microseconds),
        }
    }

    #[test]
    fn elapsed_rounded_up() {
        check!(expired_timeout(1_500).elapsed_rounded().as_micros() == 2_000);
    }

    #[test]
    fn elapsed_rounded_exact() {
        check!(expired_timeout(2_000).elapsed_rounded().as_micros() == 2_000);
    }

    #[test]
    fn elapsed_rounded_down() {
        check!(expired_timeout(2_499).elapsed_rounded().as_micros() == 2_000);
    }

    #[test]
    fn compare_timeout_never() {
        let timeout = Timeout::Never;

        check!(Timeout::Never == timeout);
        check!(future_timeout(5_000) < timeout);
        check!(pending_timeout(5_000, 500) < timeout);
        check!(pending_timeout(5_000, 5_500) < timeout);
        check!(future_timeout(0) < timeout);
        check!(expired_timeout(5_000) < timeout);

        check!(timeout == Timeout::Never);
        check!(timeout > future_timeout(5_000));
        check!(timeout > pending_timeout(5_000, 500));
        check!(timeout > pending_timeout(5_000, 5_500));
        check!(timeout > future_timeout(0));
        check!(timeout > expired_timeout(5_000));
    }

    #[test]
    fn compare_timeout_future() {
        let timeout = future_timeout(5_000);

        check!(Timeout::Never > timeout);
        check!(future_timeout(5_000) == timeout);
        check!(pending_timeout(5_000, 500) < timeout);
        check!(pending_timeout(5_000, 5_500) < timeout);
        check!(future_timeout(0) < timeout);
        check!(expired_timeout(5_000) < timeout);

        check!(timeout < Timeout::Never);
        check!(timeout == future_timeout(5_000));
        check!(timeout > pending_timeout(5_000, 500));
        check!(timeout > pending_timeout(5_000, 5_500));
        check!(timeout > future_timeout(0));
        check!(timeout > expired_timeout(5_000));
    }

    #[test]
    fn compare_timeout_pending() {
        let timeout = pending_timeout(5_000, 1000);

        check!(Timeout::Never > timeout);
        check!(future_timeout(5_000) > timeout);
        check!(pending_timeout(5_000, 500) > timeout);
        check!(pending_timeout(5_000, 5_500) < timeout);
        check!(future_timeout(0) < timeout);
        check!(expired_timeout(5_000) < timeout);

        check!(timeout < Timeout::Never);
        check!(timeout < future_timeout(5_000));
        check!(timeout < pending_timeout(5_000, 500));
        check!(timeout > pending_timeout(5_000, 5_500));
        check!(timeout > future_timeout(0));
        check!(timeout > expired_timeout(5_000));
    }

    #[test]
    fn compare_timeout_pending_overtime() {
        let timeout = pending_timeout(5_000, 6_000);

        check!(Timeout::Never > timeout);
        check!(future_timeout(5_000) > timeout);
        check!(pending_timeout(5_000, 500) > timeout);
        check!(pending_timeout(5_000, 5_500) == timeout);
        check!(future_timeout(0) == timeout);
        check!(expired_timeout(5_000) == timeout);

        check!(timeout < Timeout::Never);
        check!(timeout < future_timeout(5_000));
        check!(timeout < pending_timeout(5_000, 500));
        check!(timeout == pending_timeout(5_000, 5_500));
        check!(timeout == future_timeout(0));
        check!(timeout == expired_timeout(5_000));
    }

    #[test]
    fn compare_timeout_expired() {
        let timeout = expired_timeout(5_000);

        check!(Timeout::Never > timeout);
        check!(future_timeout(5_000) > timeout);
        check!(pending_timeout(5_000, 500) > timeout);
        check!(pending_timeout(5_000, 5_500) == timeout);
        check!(future_timeout(0) == timeout);
        check!(expired_timeout(5_000) == timeout);

        check!(timeout < Timeout::Never);
        check!(timeout < future_timeout(5_000));
        check!(timeout < pending_timeout(5_000, 500));
        check!(timeout == pending_timeout(5_000, 5_500));
        check!(timeout == future_timeout(0));
        check!(timeout == expired_timeout(5_000));
    }
}
