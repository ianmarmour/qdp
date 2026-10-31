//! What `qdp connect` does with decoded frames, until it has a window.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use qdp_codec::Yuv420Frame;
use qdp_runtime::{FrameInfo, FrameSink, SinkControl};
use tracing::{info, warn};

/// How often statistics are logged.
const STATS_INTERVAL: Duration = Duration::from_secs(1);

/// The frame rate written in the YUV4MPEG2 header. The file has no timestamps,
/// so this only sets the playback speed.
const Y4M_FRAME_RATE: u32 = 60;

/// Writes frames to a YUV4MPEG2 file, which most video players and ffmpeg read.
struct Y4mWriter {
    /// The file.
    output: BufWriter<File>,
    /// The frame size written in the header, once the first frame arrived.
    size: Option<(usize, usize)>,
}

impl Y4mWriter {
    /// Creates the file.
    fn create(path: &Path) -> io::Result<Self> {
        Ok(Self {
            output: BufWriter::new(File::create(path)?),
            size: None,
        })
    }

    /// Appends one frame, writing the header first. Frames of another size than
    /// the first are skipped, because the format cannot change size.
    fn write(&mut self, frame: &Yuv420Frame) -> io::Result<()> {
        let size = (frame.width(), frame.height());
        match self.size {
            None => {
                writeln!(
                    self.output,
                    "YUV4MPEG2 W{} H{} F{Y4M_FRAME_RATE}:1 Ip A1:1 C420jpeg",
                    size.0, size.1
                )?;
                self.size = Some(size);
            }
            Some(first) if first != size => return Ok(()),
            Some(_) => {}
        }
        self.output.write_all(b"FRAME\n")?;
        self.output.write_all(frame.y())?;
        self.output.write_all(frame.u())?;
        self.output.write_all(frame.v())?;
        Ok(())
    }
}

/// The frames presented since statistics were last logged.
struct StatsWindow {
    /// When the window started.
    started: Instant,
    /// The frames presented in the window.
    frames: u64,
    /// The summed latency of the frames whose latency is known.
    latency_sum: Duration,
    /// The number of frames whose latency is known.
    timed_frames: u32,
}

impl StatsWindow {
    /// Starts a window at `started`.
    fn new(started: Instant) -> Self {
        Self {
            started,
            frames: 0,
            latency_sum: Duration::ZERO,
            timed_frames: 0,
        }
    }
}

/// Counts frames, logs statistics, optionally writes the video to a file, and
/// stops after a frame limit.
pub struct CliSink {
    /// Where the video is written, if anywhere.
    output: Option<Y4mWriter>,
    /// The number of frames after which to stop.
    frame_limit: Option<u64>,
    /// The frames presented so far.
    frames: u64,
    /// The statistics since they were last logged.
    window: StatsWindow,
}

impl CliSink {
    /// Creates the sink, opening `output` if given.
    pub fn new(output: Option<&Path>, frame_limit: Option<u64>) -> io::Result<Self> {
        Ok(Self {
            output: output.map(Y4mWriter::create).transpose()?,
            frame_limit,
            frames: 0,
            window: StatsWindow::new(Instant::now()),
        })
    }

    /// Returns the frames presented so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Flushes the output file.
    pub fn finish(&mut self) -> io::Result<()> {
        match &mut self.output {
            Some(writer) => writer.output.flush(),
            None => Ok(()),
        }
    }

    /// Logs the statistics of the last interval if it has ended.
    fn log_stats(&mut self, now: Instant, frame: &Yuv420Frame) {
        let elapsed = now.saturating_duration_since(self.window.started);
        if elapsed < STATS_INTERVAL {
            return;
        }
        let fps = self.window.frames as f64 / elapsed.as_secs_f64();
        let latency = (self.window.timed_frames > 0)
            .then(|| self.window.latency_sum / self.window.timed_frames);
        info!(
            fps = format!("{fps:.1}"),
            latency_ms = latency.map(|latency| format!("{:.1}", latency.as_secs_f64() * 1000.0)),
            width = frame.width(),
            height = frame.height(),
            total = self.frames,
            "receiving"
        );
        self.window = StatsWindow::new(now);
    }
}

impl FrameSink for CliSink {
    fn present(&mut self, frame: &Yuv420Frame, info: FrameInfo) -> SinkControl {
        self.frames += 1;
        self.window.frames += 1;
        if let Some(latency) = info.latency {
            self.window.latency_sum += latency;
            self.window.timed_frames += 1;
        }
        if let Some(writer) = &mut self.output {
            if let Err(error) = writer.write(frame) {
                warn!(%error, "writing the video failed; no more frames are written");
                self.output = None;
            }
        }
        self.log_stats(info.decoded_at, frame);
        match self.frame_limit {
            Some(limit) if self.frames >= limit => SinkControl::Stop,
            _ => SinkControl::Continue,
        }
    }
}
