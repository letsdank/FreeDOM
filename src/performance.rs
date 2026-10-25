use std::time::Duration;

const REPORT_INTERVAL: u32 = 60;

pub(crate) struct FrameProfiler {
    enabled: bool,
    frame_count: u32,
    paint_total: Duration,
    paint_slowest: Duration,
    present_total: Duration,
    present_slowest: Duration,
}

impl FrameProfiler {
    pub(crate) fn from_environment() -> Self {
        Self {
            enabled: std::env::var_os("FREEDOM_PROFILE_RENDER").is_some(),
            frame_count: 0,
            paint_total: Duration::ZERO,
            paint_slowest: Duration::ZERO,
            present_total: Duration::ZERO,
            present_slowest: Duration::ZERO,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn record(&mut self, paint: Duration, present: Duration) {
        if !self.enabled {
            return;
        }

        self.frame_count += 1;
        self.paint_total += paint;
        self.present_total += present;
        self.paint_slowest = self.paint_slowest.max(paint);
        self.present_slowest = self.present_slowest.max(present);

        if self.frame_count < REPORT_INTERVAL {
            return;
        }

        let count = f64::from(self.frame_count);

        eprintln!(
            "FreeDOM render: {frames} frames, paint avg {paint_avg:.2} ms max {paint_max:.2} ms, present avg {present_avg:.2} ms max {present_max:.2} ms",
            frames = self.frame_count,
            paint_avg = self.paint_total.as_secs_f64() * 1_000.0 / count,
            paint_max = self.paint_slowest.as_secs_f64() * 1_000.0,
            present_avg = self.present_total.as_secs_f64() * 1_000.0 / count,
            present_max = self.present_slowest.as_secs_f64() * 1_000.0,
        );

        self.frame_count = 0;
        self.paint_total = Duration::ZERO;
        self.paint_slowest = Duration::ZERO;
        self.present_total = Duration::ZERO;
        self.present_slowest = Duration::ZERO;
    }
}
