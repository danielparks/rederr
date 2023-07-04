//! # Stateful timeouts
//!
//! [`Timeout`] provides a way to keep track of timeout that may or may not have
//! started. It simplifies managing long running timeouts, particularly when
//! they cover a number of function calls that each have their own timeouts.
//!
//! For example, you might want to allow an overall timeout for an entire run of
//! your application. If your application makes calls that have their own
//! timeouts, such as reading from a [`std::net::TcpStream`], you will need to
//! set the timeout for the read correctly so that you don’t exceed the overall
//! timeout.

use std::cmp::Ordering;
use std::fmt;
use std::time::{Duration, Instant};

/// Minimum valid timeout that poll() respects.
const TIMEOUT_RESOLUTION: Duration = Duration::from_millis(1);

/// A stateful timeout.
///
/// Create a `Timeout::Future` to represent a planned timeout. Run
/// [`Timeout::start()`] to get a new `Timeout::Pending` that tracks how much
/// time has passed, then call [`Timeout::check_expired()`] on that to get
/// `Timeout::Expired` when the timeout has expired.
#[derive(Clone, Eq, Debug)]
pub enum Timeout {
    /// Never time out.
    Never,

    /// Time out after `timeout` has elapsed.
    ///
    /// It’s probably most convenient to use [`Timeout::from()`] to create a
    /// timeout. For example:
    ///
    /// ```rust
    /// use assert2::let_assert;
    /// use cron_wrapper::timeout::Timeout;
    /// use std::time::Duration;
    ///
    /// let_assert!(
    ///     Timeout::Future { .. } = Timeout::from(Duration::from_millis(100))
    /// );
    /// ```
    Future {
        /// The length of the timeout.
        timeout: Duration,
    },

    /// A timeout that is counting down.
    ///
    /// Produced by [`Timeout::start()`].
    Pending {
        /// The length of the timeout.
        timeout: Duration,

        /// When the timeout started.
        start: Instant,
    },

    /// A timeout that has expired.
    ///
    /// Produced by [`Timeout::check_expired()`].
    Expired {
        /// The original length of the timeout.
        requested: Duration,

        /// How much time actually elapsed before the operation was canceled.
        actual: Duration,
    },
}

impl Timeout {
    /// Get the remaining timeout if available.
    ///
    /// Returns `Some(Duration::ZERO)` if the timeout has already expired.
    #[must_use]
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
    #[must_use]
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
    ///
    /// Returns:
    ///   * `None` if the timeout has not expired.
    ///   * `Some(Timeout::Expired { .. })` if the timeout has expired.
    #[must_use]
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

    /// Calculate how much of the timeout has elapsed.
    ///
    /// [`Timeout::Never`] and [`Timeout::Future`] both always return
    /// [`Duration::ZERO`].
    ///
    /// This will not do anything special if called on a [`Timeout::Pending`]
    /// that has expired. See [`Timeout::check_expired()`].
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        match &self {
            Self::Never | Self::Future { .. } => Duration::ZERO,
            Self::Pending { start, .. } => start.elapsed(),
            Self::Expired { actual, .. } => *actual,
        }
    }

    /// Calculate how much of the timeout has elapsed, rounded to the nearest
    /// millisecond.
    ///
    /// [`Timeout::Never`] and [`Timeout::Future`] both always return
    /// [`Duration::ZERO`].
    ///
    /// This will not do anything special if called on a [`Timeout::Pending`]
    /// that has expired. See [`Timeout::check_expired()`].
    #[must_use]
    pub fn elapsed_rounded(&self) -> Duration {
        // FIXME: actually consult resolution?
        let elapsed = self.elapsed();
        let nanos: u32 = elapsed.subsec_nanos();
        let sub_ms = nanos % 1_000_000;

        // sub_ms is nanos % 1e6, so sub_ms <= nanos
        #[allow(clippy::arithmetic_side_effects)]
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
        timeout.map(Self::from).unwrap_or(Self::Never)
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
    // This triggers for the various compare_ tests.
    #![allow(clippy::cognitive_complexity)]

    use super::*;
    use assert2::{check, let_assert};
    use std::time::Duration;

    const fn future_timeout(microseconds: u64) -> Timeout {
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

    const fn expired_timeout(microseconds: u64) -> Timeout {
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

    #[test]
    fn check_expired_timeout_never() {
        check!(Timeout::Never.check_expired() == None);
    }

    #[test]
    fn check_expired_timeout_future() {
        check!(future_timeout(1_000).check_expired() == None);
    }

    #[test]
    fn check_expired_timeout_pending() {
        check!(pending_timeout(5_000, 1_000).check_expired() == None);
    }

    #[test]
    fn check_expired_timeout_pending_overtime() {
        let_assert!(
            Some(Timeout::Expired { .. }) =
                pending_timeout(5_000, 6_000).check_expired()
        );
    }

    #[test]
    fn check_expired_timeout_expired() {
        let timeout = expired_timeout(5_000);
        check!(timeout.check_expired() == Some(timeout));
    }
}
