//! Implementation of the abstract clock for the linux platform (and macOS)

#[cfg(target_os = "linux")]
use std::path::Path;

use clock_steering::{unix::UnixClock, TimeOffset};
#[cfg(target_os = "linux")]
use statime::config::LeapIndicator;
use crate::config::SystemClockType;
use statime::{
    config::TimePropertiesDS,
    overlay_clock::{ClockOverlayExporter, OverlayClock},
    time::{Duration, Time},
    Clock, SharedClock,
};

#[derive(Debug, Clone)]
pub struct LinuxClock {
    clock: clock_steering::unix::UnixClock,
    #[cfg(target_os = "linux")]
    is_tai: bool,
}

/// Map a configured virtual system clock base to its clock id.
pub fn base_clockid(base: SystemClockType) -> libc::clockid_t {
    match base {
        #[cfg(target_os = "linux")]
        SystemClockType::TAI => libc::CLOCK_TAI,
        #[cfg(target_os = "linux")]
        SystemClockType::Monotonic => libc::CLOCK_MONOTONIC,
        #[cfg(target_os = "linux")]
        SystemClockType::MonotonicRaw => libc::CLOCK_MONOTONIC_RAW,
        #[cfg(target_os = "linux")]
        SystemClockType::MonotonicCoarse => libc::CLOCK_MONOTONIC_COARSE,
        #[cfg(target_os = "macos")]
        SystemClockType::UptimeRaw => libc::CLOCK_UPTIME_RAW,
        #[allow(unreachable_patterns)]
        other => unreachable!("{other:?} rejected by validate_platform"),
    }
}

impl LinuxClock {
    #[cfg(target_os = "linux")]
    pub const CLOCK_TAI: Self = Self {
        clock: UnixClock::CLOCK_TAI,
        is_tai: true,
    };

    /// The clock that software (socket) timestamps are expressed in, used as
    /// the system clock: CLOCK_TAI on Linux, CLOCK_REALTIME on macOS (which has
    /// no TAI clock; the TAI offset is treated as zero there).
    pub fn system_timestamp_clock() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::CLOCK_TAI
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::from_clockid(libc::CLOCK_REALTIME)
        }
    }

    /// Cheap monotonic clock for the usrvclock export guard.
    pub fn guard_monotonic_clock() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::from_clockid(libc::CLOCK_MONOTONIC_COARSE)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::from_clockid(libc::CLOCK_MONOTONIC)
        }
    }

    pub fn from_clockid(clockid: libc::clockid_t) -> Self {
        Self {
            clock: UnixClock::from_clockid(clockid),
            #[cfg(target_os = "linux")]
            is_tai: clockid == libc::CLOCK_TAI,
        }
    }

    #[cfg(target_os = "linux")]
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let clock = UnixClock::open(path)?;

        Ok(Self {
            clock,
            is_tai: false,
        })
    }

    // This forces the clock to start with seconds such that the current time in
    // nanoseconds is representable as a u64.
    pub fn init(&mut self) -> Result<(), clock_steering::unix::Error> {
        use clock_steering::Clock;

        let ts = self.clock.now()?;
        #[allow(clippy::unnecessary_cast)]
        if ts.seconds < 0 || ts.seconds as i64 > (u64::MAX / 1000000000) as i64 {
            self.clock.step_clock(TimeOffset {
                seconds: -ts.seconds + 1,
                nanos: 0,
            })?;
        }

        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn open_idx(idx: u32) -> std::io::Result<Self> {
        let path = format!("/dev/ptp{}", idx);
        Self::open(path)
    }

    /// Return three timestamps t1 t2 and t3 minted in that order.
    /// T1 and T3 are minted using the system TAI clock and T2 by the hardware
    /// clock
    #[cfg(target_os = "linux")]
    pub fn system_offset(&self) -> Result<(Time, Time, Time), clock_steering::unix::Error> {
        // The clock crate's system offset gives the T1 and T3 timestamps on the
        // CLOCK_REALTIME timescale which is UTC, not TAI, so we need to correct
        // here.
        // FIXME: really?
        self.clock.system_offset().map(|(mut t1, t2, mut t3)| {
            use clock_steering::Clock;
            let tai_offset = UnixClock::CLOCK_REALTIME.get_tai().unwrap();
            t1.seconds += tai_offset as libc::time_t;
            t3.seconds += tai_offset as libc::time_t;
            // FIXME: really?
            (
                clock_timestamp_to_time(t1),
                clock_timestamp_to_time(t2),
                clock_timestamp_to_time(t3),
            )
        })
    }

    #[cfg(target_os = "linux")]
    pub fn get_tai_offset(&self) -> Result<i32, clock_steering::unix::Error> {
        use clock_steering::Clock;
        if self.is_tai {
            UnixClock::CLOCK_REALTIME.get_tai()
        } else {
            self.clock.get_tai()
        }
    }

    /// macOS has no kernel TAI offset; all clocks are used as-is.
    #[cfg(not(target_os = "linux"))]
    pub fn get_tai_offset(&self) -> Result<i32, clock_steering::unix::Error> {
        Ok(0)
    }
}

#[cfg(target_os = "linux")]
fn clock_timestamp_to_time(t: clock_steering::Timestamp) -> Time {
    Time::from_nanos((t.seconds as u64) * 1_000_000_000 + (t.nanos as u64))
}

fn time_from_timestamp(timestamp: clock_steering::Timestamp, fallback: Time) -> Time {
    let Ok(seconds): Result<u64, _> = timestamp.seconds.try_into() else {
        return fallback;
    };

    let nanos = seconds * 1_000_000_000 + timestamp.nanos as u64;
    Time::from_nanos_subnanos(nanos, 0)
}

impl Clock for LinuxClock {
    type Error = clock_steering::unix::Error;

    fn now(&self) -> Time {
        use clock_steering::Clock;

        let timestamp = self.clock.now().unwrap();
        time_from_timestamp(timestamp, Time::from_fixed_nanos(0))
    }

    fn set_frequency(&mut self, freq: f64) -> Result<Time, Self::Error> {
        use clock_steering::Clock;
        log::trace!("Setting clock frequency to {:e}ppm", freq);
        #[cfg(target_os = "linux")]
        let timestamp = if self.is_tai {
            // Clock tai can't directly adjust frequency, so drive this through
            // clock_realtime and adjust the received timestamp
            let mut ts = UnixClock::CLOCK_REALTIME.set_frequency(freq)?;
            ts.seconds += UnixClock::CLOCK_REALTIME.get_tai()? as libc::time_t;
            ts
        } else {
            self.clock.set_frequency(freq)?
        };
        #[cfg(not(target_os = "linux"))]
        let timestamp = self.clock.set_frequency(freq)?;
        Ok(time_from_timestamp(timestamp, statime::Clock::now(self)))
    }

    fn step_clock(&mut self, time_offset: Duration) -> Result<Time, Self::Error> {
        use clock_steering::Clock;

        // Since we want nanos to be in [0,1_000_000_000), we need
        // euclidean division and remainder.
        let offset_nanos: i128 = time_offset.nanos_rounded();
        let offset = TimeOffset {
            seconds: offset_nanos
                .div_euclid(1_000_000_000)
                .try_into()
                .expect("Unexpected jump larger than 2^64 seconds"),
            nanos: offset_nanos.rem_euclid(1_000_000_000) as _, // Result will always fit in u32
        };

        log::trace!(
            "Stepping clock {:e}ns",
            (offset.seconds as f64) * 1e9 + (offset.nanos as f64)
        );

        #[cfg(target_os = "linux")]
        let timestamp = if self.is_tai {
            // Clock tai can't directly step, so drive this through clock_realtime
            // and adjust the received timestamp
            let mut ts = UnixClock::CLOCK_REALTIME.step_clock(offset)?;
            ts.seconds += UnixClock::CLOCK_REALTIME.get_tai()? as libc::time_t;
            ts
        } else {
            self.clock.step_clock(offset)?
        };
        #[cfg(not(target_os = "linux"))]
        let timestamp = self.clock.step_clock(offset)?;
        Ok(time_from_timestamp(timestamp, statime::Clock::now(self)))
    }

    #[cfg(not(target_os = "linux"))]
    fn set_properties(&mut self, _time_properties: &TimePropertiesDS) -> Result<(), Self::Error> {
        // Never touch the kernel clock's TAI offset or leap state on macOS.
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn set_properties(&mut self, time_properties: &TimePropertiesDS) -> Result<(), Self::Error> {
        use clock_steering::Clock;

        // These properties should always be communicated to the system clock.

        if let Some(offset) = time_properties.utc_offset() {
            UnixClock::CLOCK_REALTIME.set_tai(offset as _)?;
        }

        UnixClock::CLOCK_REALTIME.set_leap_seconds(match time_properties.leap_indicator() {
            LeapIndicator::NoLeap => clock_steering::LeapIndicator::NoWarning,
            LeapIndicator::Leap61 => clock_steering::LeapIndicator::Leap61,
            LeapIndicator::Leap59 => clock_steering::LeapIndicator::Leap59,
        })?;

        Ok(())
    }
}

pub fn libc_timespec_into_instant(spec: libc::timespec) -> Time {
    Time::from_fixed_nanos(spec.tv_sec as i128 * 1_000_000_000i128 + spec.tv_nsec as i128)
}

pub trait PortTimestampToTime {
    fn port_timestamp_to_time(&self, ts: timestamped_socket::socket::Timestamp) -> Time;
}

impl PortTimestampToTime for LinuxClock {
    fn port_timestamp_to_time(&self, mut ts: timestamped_socket::socket::Timestamp) -> Time {
        // get_tai gives zero if this is a hardware clock, and the needed
        // correction when this port uses software timestamping
        ts.seconds += self.get_tai_offset().expect("Unable to get tai offset") as i64;
        Time::from_fixed_nanos(ts.seconds as i128 * 1_000_000_000i128 + ts.nanos as i128)
    }
}

impl<T: PortTimestampToTime + Clock, E: ClockOverlayExporter + core::fmt::Debug> PortTimestampToTime
    for OverlayClock<T, E>
{
    fn port_timestamp_to_time(&self, ts: timestamped_socket::socket::Timestamp) -> Time {
        let roclock_time = self.underlying().port_timestamp_to_time(ts);
        self.time_from_underlying(roclock_time)
    }
}

impl<T: PortTimestampToTime + Clock> PortTimestampToTime
    for SharedClock<T>
{
    fn port_timestamp_to_time(&self, ts: timestamped_socket::socket::Timestamp) -> Time {
        self.0.lock().unwrap().port_timestamp_to_time(ts)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_tai_offset_is_zero_and_set_properties_is_noop() {
        let mut c = LinuxClock::system_timestamp_clock();
        assert_eq!(c.get_tai_offset().unwrap(), 0);
        let props = statime::config::TimePropertiesDS::new_arbitrary_time(false, false, statime::config::TimeSource::InternalOscillator);
        assert!(statime::Clock::set_properties(&mut c, &props).is_ok());
    }
}
