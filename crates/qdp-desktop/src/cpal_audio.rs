//! Audio on macOS and Windows through cpal: speakers and microphones, and on
//! Windows the desktop's sound, through WASAPI loopback capture of the default
//! output.
//!
//! Streams play at [`SAMPLE_RATE`], which the platforms convert to the
//! device's rate. Capture takes the device's own rate and channels, which the
//! input converts.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use qdp_runtime::{ActiveDevice, AudioInput, AudioOutput, CaptureSink, Playout, SAMPLE_RATE};
use tracing::warn;

/// The channels a client plays the host's audio in.
const SPEAKER_CHANNELS: u16 = 2;

/// The channels a microphone is sent in.
const MICROPHONE_CHANNELS: u16 = 1;

/// The channels the desktop's sound is captured in.
const DESKTOP_CHANNELS: u16 = 2;

/// The largest value of a 16-bit sample.
const I16_SCALE: f32 = 32_768.0;

/// Converts audio at one sample rate to another by linear interpolation,
/// across the chunks a device delivers.
#[derive(Debug, Clone)]
struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// The position of the next output sample, in input samples after the
    /// last frame of the previous chunk.
    position: f64,
    /// The last frame of the previous chunk, one sample per channel.
    previous: Vec<f32>,
}

impl Resampler {
    /// Returns a converter from `from` hertz to `to` hertz of `channels`
    /// channels.
    fn new(from: u32, to: u32, channels: usize) -> Self {
        Self {
            step: f64::from(from) / f64::from(to),
            position: 1.0,
            previous: vec![0.0; channels],
        }
    }

    /// Converts one chunk of interleaved frames.
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let channels = self.previous.len();
        let frames = input.len() / channels;
        if self.step == 1.0 {
            return input.to_vec();
        }
        // Frame 0 is the previous chunk's last; frame n is the input's n - 1.
        let frame = |index: usize, channel: usize| {
            if index == 0 {
                self.previous[channel]
            } else {
                input[(index - 1) * channels + channel]
            }
        };
        let mut output = Vec::new();
        while self.position <= frames as f64 {
            let index = self.position as usize;
            let fraction = (self.position - index as f64) as f32;
            for channel in 0..channels {
                let current = frame(index, channel);
                let next = if index < frames {
                    frame(index + 1, channel)
                } else {
                    current
                };
                output.push(current + (next - current) * fraction);
            }
            self.position += self.step;
        }
        if frames > 0 {
            self.previous
                .copy_from_slice(&input[(frames - 1) * channels..frames * channels]);
            self.position -= frames as f64;
        }
        output
    }
}

/// Returns an error message for a cpal failure.
fn message(context: &str, error: impl std::fmt::Display) -> String {
    format!("{context}: {error}")
}

/// What an input captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The default microphone.
    Microphone,
    /// What plays on the default output, through loopback capture.
    #[cfg_attr(not(windows), allow(dead_code))]
    Loopback,
}

/// Captures audio through cpal.
#[derive(Debug)]
struct CpalInput {
    /// What it captures.
    source: Source,
    /// The channels it delivers.
    channels: u16,
}

impl AudioInput for CpalInput {
    fn channels(&self) -> u16 {
        self.channels
    }

    fn start(&mut self, capture: CaptureSink) -> Result<ActiveDevice, String> {
        let host = cpal::default_host();
        let device = match self.source {
            Source::Microphone => host.default_input_device(),
            Source::Loopback => host.default_output_device(),
        }
        .ok_or("there is no audio device")?;
        let supported = match self.source {
            Source::Microphone => device.default_input_config(),
            Source::Loopback => device.default_output_config(),
        }
        .map_err(|error| message("cannot read the audio device's format", error))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let device_channels = usize::from(config.channels);
        let wanted = usize::from(self.channels);
        let mut resampler = Resampler::new(config.sample_rate, SAMPLE_RATE, wanted);
        let mut deliver = move |samples: &[f32]| {
            let remixed: Vec<f32> = samples
                .chunks_exact(device_channels)
                .flat_map(|frame| {
                    let mono = frame.iter().sum::<f32>() / device_channels as f32;
                    (0..wanted).map(move |channel| match wanted {
                        1 => mono,
                        _ => frame.get(channel).copied().unwrap_or(frame[0]),
                    })
                })
                .collect();
            capture.push(&resampler.process(&remixed));
        };
        let error = |error: cpal::Error| warn!(%error, "audio capture failed");
        let stream = match format {
            SampleFormat::F32 => device.build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| deliver(data),
                error,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> = data
                        .iter()
                        .map(|&sample| f32::from(sample) / I16_SCALE)
                        .collect();
                    deliver(&samples);
                },
                error,
                None,
            ),
            other => {
                return Err(format!(
                    "the audio device's {other} samples are not supported"
                ));
            }
        }
        .map_err(|error| message("cannot capture audio", error))?;
        stream
            .play()
            .map_err(|error| message("cannot start capturing audio", error))?;
        Ok(ActiveDevice(Box::new(stream)))
    }

    fn description(&self) -> &str {
        match self.source {
            Source::Microphone => "microphone",
            Source::Loopback => "desktop audio (loopback)",
        }
    }
}

/// Plays audio through cpal on the default output.
#[derive(Debug)]
struct CpalOutput {
    /// The most channels it plays.
    channels: u16,
}

impl AudioOutput for CpalOutput {
    fn channels(&self) -> u16 {
        self.channels
    }

    fn start(&mut self, channels: u16, playout: Playout) -> Result<ActiveDevice, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("there is no audio output device")?;
        let config = StreamConfig {
            channels,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };
        let stream = device
            .build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| playout.pull(data),
                |error: cpal::Error| warn!(%error, "audio playback failed"),
                None,
            )
            .map_err(|error| message("cannot play audio", error))?;
        stream
            .play()
            .map_err(|error| message("cannot start playing audio", error))?;
        Ok(ActiveDevice(Box::new(stream)))
    }

    fn description(&self) -> &str {
        "speakers"
    }
}

/// Returns the default output device as speakers, if there is one.
pub(crate) fn speakers() -> Option<Box<dyn AudioOutput>> {
    cpal::default_host().default_output_device()?;
    Some(Box::new(CpalOutput {
        channels: SPEAKER_CHANNELS,
    }))
}

/// Returns the default microphone, if there is one.
pub(crate) fn microphone() -> Option<Box<dyn AudioInput>> {
    cpal::default_host().default_input_device()?;
    Some(Box::new(CpalInput {
        source: Source::Microphone,
        channels: MICROPHONE_CHANNELS,
    }))
}

/// Returns the desktop's sound: loopback capture of the default output.
#[cfg(windows)]
pub(crate) fn desktop_input() -> Option<Box<dyn AudioInput>> {
    cpal::default_host().default_output_device()?;
    Some(Box::new(CpalInput {
        source: Source::Loopback,
        channels: DESKTOP_CHANNELS,
    }))
}

/// Returns the desktop's sound: macOS captures it with ScreenCaptureKit
/// rather than cpal.
#[cfg(target_os = "macos")]
pub(crate) fn desktop_input() -> Option<Box<dyn AudioInput>> {
    let _ = DESKTOP_CHANNELS;
    crate::macos::desktop_audio()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Converting 24 kHz to 48 kHz doubles the samples and interpolates
    /// between them, across chunks.
    #[test]
    fn resamples() {
        let mut up = Resampler::new(24_000, 48_000, 1);
        let first = up.process(&[0.0, 1.0]);
        let second = up.process(&[1.0, 0.0]);
        assert_eq!(first, [0.0, 0.5, 1.0]);
        assert_eq!(second, [1.0, 1.0, 0.5, 0.0]);
        let mut same = Resampler::new(48_000, 48_000, 2);
        assert_eq!(same.process(&[0.1, 0.2]), [0.1, 0.2]);
        let mut down = Resampler::new(96_000, 48_000, 1);
        let total: usize = (0..10).map(|_| down.process(&[0.0; 960]).len()).sum();
        assert!((479..=481).contains(&(total / 10)), "{total}");
    }
}
