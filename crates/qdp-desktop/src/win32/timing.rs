//! The display's refresh timing, from the Desktop Window Manager, which
//! composes every window at the display's refresh (Section 5.3).

use std::time::{Duration, Instant};

use qdp_runtime::{DisplayTiming, RefreshMode};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// Returns the refresh period of the display the desktop is composed for,
/// and its last vertical blank, or `None` if composition timing is not
/// available.
pub fn display_timing() -> Option<DisplayTiming> {
    let mut info = DWM_TIMING_INFO {
        cbSize: size_of::<DWM_TIMING_INFO>() as u32,
        ..DWM_TIMING_INFO::default()
    };
    // SAFETY: `info` is valid for writes and its size is set, as the call
    // requires; a null window asks about the whole desktop's composition.
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) }.ok()?;
    let mut frequency = 0;
    let mut counter = 0;
    // SAFETY: both outputs are valid for writes.
    unsafe { QueryPerformanceFrequency(&mut frequency) }.ok()?;
    // SAFETY: as above.
    unsafe { QueryPerformanceCounter(&mut counter) }.ok()?;
    let now = Instant::now();
    let rate = info.rateRefresh;
    if rate.uiNumerator == 0 || frequency <= 0 {
        return None;
    }
    let refresh_period =
        Duration::from_secs_f64(f64::from(rate.uiDenominator) / f64::from(rate.uiNumerator));
    let since_vblank = u64::try_from(counter).ok()?.checked_sub(info.qpcVBlank)?;
    let since_vblank = Duration::from_secs_f64(since_vblank as f64 / frequency as f64);
    Some(DisplayTiming {
        refresh_period,
        vsync: now.checked_sub(since_vblank)?,
        mode: RefreshMode::Fixed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The longest refresh period that is plausible for a display.
    const LONGEST_PERIOD: Duration = Duration::from_secs(1);

    /// Composition timing, where the system gives it, is a plausible period
    /// and a vertical blank in the past.
    #[test]
    fn reads_composition_timing() {
        let Some(timing) = display_timing() else {
            return;
        };
        assert!(!timing.refresh_period.is_zero() && timing.refresh_period < LONGEST_PERIOD);
        assert!(timing.vsync <= Instant::now());
    }
}
