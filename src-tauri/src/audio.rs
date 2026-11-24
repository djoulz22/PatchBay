//! Native audio endpoints. The routing engine owns the consumer of every input
//! and the producer of every output. Core Audio owns the opposite ends.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

use cpal::{
    Device, FromSample, Host, SampleFormat, SizedSample, Stream, StreamConfig,
    SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use serde::Serialize;

use crate::{browser_audio::BrowserCapture, graph::NodeKind};

#[cfg(target_os = "macos")]
mod macos_tap;
#[cfg(target_os = "macos")]
pub(crate) use macos_tap::device_object;

const BUFFER_FRAMES: usize = 4096;
const MAX_CHANNELS: usize = 64;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub channels: usize,
    pub sample_rate: u32,
    pub is_default: bool,
    pub is_virtual: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioApplication {
    pub bundle_id: String,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioCatalog {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
    pub applications: Vec<AudioApplication>,
    pub system_audio: bool,
}

#[derive(Clone, Copy)]
enum Direction {
    Input,
    Output,
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Self::Input => "Entrée audio",
            Self::Output => "Sortie audio",
        }
    }

    fn default_device(self, host: &Host) -> Option<Device> {
        match self {
            Self::Input => host.default_input_device(),
            Self::Output => host.default_output_device(),
        }
    }

    fn default_config(self, device: &Device) -> Option<SupportedStreamConfig> {
        match self {
            Self::Input => device.default_input_config().ok(),
            Self::Output => device.default_output_config().ok(),
        }
    }

    fn describe(
        self,
        device: &Device,
        default_id: Option<&str>,
        requested_rate: u32,
    ) -> Option<(AudioDevice, SupportedStreamConfig)> {
        let id = device.id().ok()?.1;
        let description = device.description().ok()?;
        let name = description.name().to_owned();
        let default_config = self.default_config(device);
        #[cfg(target_os = "macos")]
        // CPAL changes the device's nominal rate when building an input stream at a
        // different rate. Keep the HAL's current rate and resample in the engine.
        let requested_rate = {
            let _ = requested_rate;
            macos_tap::device_sample_rate(&id)?
        };
        let supported = match self {
            Self::Input => device.supported_input_configs().ok().map(Iterator::collect),
            Self::Output => device
                .supported_output_configs()
                .ok()
                .map(Iterator::collect),
        }
        .unwrap_or_default();
        let config = choose_config(supported, default_config, requested_rate)?;
        #[cfg(target_os = "macos")]
        if config.sample_rate() != requested_rate {
            return None;
        }
        #[cfg(target_os = "macos")]
        let is_virtual = macos_tap::is_virtual_device(&id, &name);
        #[cfg(not(target_os = "macos"))]
        let is_virtual = description.device_type() == cpal::DeviceType::Virtual
            || description.interface_type() == cpal::InterfaceType::Virtual;

        Some((
            AudioDevice {
                is_default: default_id == Some(id.as_str()),
                id,
                name,
                channels: usize::from(config.channels()).min(MAX_CHANNELS),
                sample_rate: config.sample_rate(),
                is_virtual,
            },
            config,
        ))
    }
}

fn choose_config(
    ranges: Vec<cpal::SupportedStreamConfigRange>,
    default_config: Option<SupportedStreamConfig>,
    requested_rate: u32,
) -> Option<SupportedStreamConfig> {
    let options = ranges
        .into_iter()
        .filter_map(|range| range.try_with_sample_rate(requested_rate));
    let fallback = default_config.into_iter();
    options
        .chain(fallback)
        .filter(|config| config.channels() != 0 && supported_format(config.sample_format()))
        .max_by_key(|config| {
            (
                config.sample_rate() == requested_rate,
                config.channels(),
                config.sample_format() == SampleFormat::F32,
            )
        })
}

fn supported_format(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32
            | SampleFormat::F64
            | SampleFormat::I16
            | SampleFormat::I32
            | SampleFormat::U16
            | SampleFormat::U32
    )
}

fn list_devices(host: &Host, direction: Direction) -> Result<Vec<AudioDevice>, String> {
    let default_id = direction
        .default_device(host)
        .and_then(|device| device.id().ok());
    let default_id = default_id.as_ref().map(|id| id.1.as_str());
    #[cfg(target_os = "macos")]
    // CPAL's direction filters and device description probe AudioUnits on macOS.
    // Reading HAL metadata keeps a catalog refresh independent of stream setup.
    let devices = host.devices();
    #[cfg(not(target_os = "macos"))]
    let devices = match direction {
        Direction::Input => host
            .input_devices()
            .map(|devices| devices.collect::<Vec<_>>()),
        Direction::Output => host
            .output_devices()
            .map(|devices| devices.collect::<Vec<_>>()),
    };
    let devices = devices.map_err(|err| format!("{} : {err}", direction.label()))?;
    let mut entries: Vec<_> = devices
        .into_iter()
        .filter_map(|device| {
            #[cfg(target_os = "macos")]
            {
                let id = device.id().ok()?.1;
                macos_tap::device_catalog_entry(id, direction, default_id)
            }
            #[cfg(not(target_os = "macos"))]
            direction
                .describe(&device, default_id, 48_000)
                .map(|(entry, _)| entry)
        })
        .filter(|entry| !entry.name.starts_with("Patchbay Audio Tap"))
        .collect();
    entries.sort_by_cached_key(|entry| (!entry.is_default, entry.name.to_lowercase()));
    Ok(entries)
}

pub(crate) fn devices() -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), String> {
    let host = cpal::default_host();
    Ok((
        list_devices(&host, Direction::Input)?,
        list_devices(&host, Direction::Output)?,
    ))
}

pub fn enumerate() -> Result<AudioCatalog, String> {
    let (inputs, outputs) = devices()?;
    #[cfg(target_os = "macos")]
    let (applications, system_audio) = (macos_tap::list_applications(), true);
    #[cfg(not(target_os = "macos"))]
    let (applications, system_audio) = (Vec::new(), false);
    Ok(AudioCatalog {
        inputs,
        outputs,
        applications,
        system_audio,
    })
}

fn find_device(
    id: &str,
    direction: Direction,
    requested_rate: u32,
) -> Result<(Device, AudioDevice, SupportedStreamConfig), String> {
    let host = cpal::default_host();
    let host_devices = host
        .devices()
        .map_err(|err| format!("Impossible de lister les périphériques audio ({err})"))?;
    for device in host_devices {
        if device.id().is_ok_and(|candidate| candidate.1 == id) {
            let (info, config) = direction
                .describe(&device, None, requested_rate)
                .filter(|(info, _)| !info.name.starts_with("Patchbay Audio Tap"))
                .ok_or_else(|| {
                    format!("{} : format audio indisponible ({id})", direction.label())
                })?;
            return Ok((device, info, config));
        }
    }
    Err(format!("{} indisponible ({id})", direction.label()))
}

/// A single-producer/single-consumer FIFO, preallocated once per endpoint.
/// Publishing a cursor with Release makes the preceding frame available to the
/// other thread. Audio callbacks do no allocation and never wait for the engine.
pub(crate) struct FrameQueue {
    samples: Box<[AtomicU32]>,
    channels: usize,
    written: AtomicUsize,
    read: AtomicUsize,
    dropped: AtomicU64,
    underruns: AtomicU64,
}

impl FrameQueue {
    pub(crate) fn new(channels: usize) -> Arc<Self> {
        Arc::new(Self {
            samples: (0..BUFFER_FRAMES * channels)
                .map(|_| AtomicU32::new(0))
                .collect(),
            channels,
            written: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        })
    }

    pub(crate) fn len(&self) -> usize {
        let read = self.read.load(Ordering::Acquire);
        let written = self.written.load(Ordering::Acquire);
        written.wrapping_sub(read)
    }

    fn slot(&self, position: usize) -> &[AtomicU32] {
        let offset = (position % BUFFER_FRAMES) * self.channels;
        &self.samples[offset..offset + self.channels]
    }

    pub(crate) fn push(&self, mut sample: impl FnMut(usize) -> f32) -> bool {
        let written = self.written.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if written.wrapping_sub(read) >= BUFFER_FRAMES {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        for (channel, cell) in self.slot(written).iter().enumerate() {
            cell.store(sample(channel).to_bits(), Ordering::Relaxed);
        }
        self.written
            .store(written.wrapping_add(1), Ordering::Release);
        true
    }

    pub(crate) fn pop(&self, mut write: impl FnMut(usize, f32)) -> bool {
        let read = self.read.load(Ordering::Relaxed);
        if self.written.load(Ordering::Acquire) == read {
            return false;
        }
        for (channel, cell) in self.slot(read).iter().enumerate() {
            write(channel, f32::from_bits(cell.load(Ordering::Relaxed)));
        }
        self.read.store(read.wrapping_add(1), Ordering::Release);
        true
    }
}

struct DeviceStream {
    _stream: Stream,
    info: AudioDevice,
    failed: Arc<AtomicBool>,
    direction: Direction,
}

impl DeviceStream {
    fn error(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        match macos_tap::device_sample_rate(&self.info.id) {
            Some(rate) if rate != self.info.sample_rate => {
                return Some(format!(
                    "{} : la fréquence de {} a changé.",
                    self.direction.label(),
                    self.info.name
                ));
            }
            None => {
                return Some(format!(
                    "{} déconnectée : {}",
                    self.direction.label(),
                    self.info.name
                ));
            }
            _ => {}
        }
        self.failed.load(Ordering::Acquire).then(|| {
            format!(
                "{} interrompue : {}",
                self.direction.label(),
                self.info.name
            )
        })
    }
}

enum InputStream {
    Device(DeviceStream),
    Browser,
    #[cfg(target_os = "macos")]
    Tap(macos_tap::SystemTap),
}

pub struct AudioInput {
    stream: InputStream,
    queue: Arc<FrameQueue>,
    sample_rate: u32,
}

impl AudioInput {
    pub fn channels(&self) -> usize {
        self.queue.channels
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn is_browser(&self) -> bool {
        matches!(self.stream, InputStream::Browser)
    }
    pub fn available_frames(&self) -> usize {
        self.queue.len()
    }
    pub fn dropped_frames(&self) -> u64 {
        self.queue.dropped.load(Ordering::Relaxed)
    }

    pub fn pop_frame(&self, frame: &mut [f32]) -> bool {
        frame.len() >= self.channels() && self.queue.pop(|channel, sample| frame[channel] = sample)
    }

    pub fn error(&self) -> Option<String> {
        match &self.stream {
            InputStream::Device(device) => device.error(),
            InputStream::Browser => None,
            #[cfg(target_os = "macos")]
            InputStream::Tap(tap) => tap.check_alive().err(),
        }
    }
}

pub struct AudioOutput {
    stream: DeviceStream,
    queue: Arc<FrameQueue>,
}

impl AudioOutput {
    pub fn channels(&self) -> usize {
        self.queue.channels
    }
    pub fn sample_rate(&self) -> u32 {
        self.stream.info.sample_rate
    }
    pub fn queued_frames(&self) -> usize {
        self.queue.len()
    }
    pub fn underruns(&self) -> u64 {
        self.queue.underruns.load(Ordering::Relaxed)
    }

    pub fn push_frame(&self, frame: &[f32]) -> bool {
        frame.len() >= self.channels() && self.queue.push(|channel| frame[channel])
    }

    pub fn error(&self) -> Option<String> {
        self.stream.error()
    }
}

pub fn open_input(
    source: &NodeKind,
    requested_rate: u32,
    browser: &BrowserCapture,
) -> Result<AudioInput, String> {
    match source {
        NodeKind::DeviceInput { device_id } => {
            let direction = Direction::Input;
            let (device, info, config) = find_device(device_id, direction, requested_rate)?;
            #[cfg(target_os = "macos")]
            // macOS applies Microphone privacy to HAL inputs, including BlackHole.
            macos_tap::authorize_microphone()
                .map_err(|error| format!("{} : {error}", info.name))?;
            let queue = FrameQueue::new(info.channels);
            let failed = Arc::new(AtomicBool::new(false));
            let stream = build_input(&device, &config, &queue, &failed)
                .map_err(|err| format!("Impossible d'ouvrir l'entrée {} ({err})", info.name))?;
            let sample_rate = info.sample_rate;
            Ok(AudioInput {
                stream: InputStream::Device(DeviceStream {
                    _stream: stream,
                    info,
                    failed,
                    direction,
                }),
                queue,
                sample_rate,
            })
        }
        NodeKind::BrowserTab { tab_id } => Ok(AudioInput {
            stream: InputStream::Browser,
            queue: browser.queue(tab_id),
            sample_rate: 48_000,
        }),
        #[cfg(target_os = "macos")]
        NodeKind::SystemAudio => open_tap(None),
        #[cfg(target_os = "macos")]
        NodeKind::Application {
            bundle_id,
            mute_original,
        } => open_tap(Some((bundle_id, *mute_original))),
        _ => Err("Cette source n'est pas disponible sur ce système.".into()),
    }
}

#[cfg(target_os = "macos")]
fn open_tap(application: Option<(&str, bool)>) -> Result<AudioInput, String> {
    let (tap, queue) = macos_tap::SystemTap::open(application)?;
    let sample_rate = tap.sample_rate();
    Ok(AudioInput {
        stream: InputStream::Tap(tap),
        queue,
        sample_rate,
    })
}

pub fn open_output(device_id: &str, requested_rate: u32) -> Result<AudioOutput, String> {
    let direction = Direction::Output;
    let (device, info, config) = find_device(device_id, direction, requested_rate)?;
    let queue = FrameQueue::new(info.channels);
    let failed = Arc::new(AtomicBool::new(false));
    let stream = build_output(&device, &config, &queue, &failed)
        .map_err(|err| format!("Impossible d'ouvrir la sortie {} ({err})", info.name))?;
    Ok(AudioOutput {
        stream: DeviceStream {
            _stream: stream,
            info,
            failed,
            direction,
        },
        queue,
    })
}

macro_rules! build_stream {
    ($builder:ident, $device:expr, $config:expr, $queue:expr, $failed:expr) => {
        match $config.sample_format() {
            SampleFormat::F32 => $builder::<f32>($device, $config.config(), $queue, $failed),
            SampleFormat::F64 => $builder::<f64>($device, $config.config(), $queue, $failed),
            SampleFormat::I16 => $builder::<i16>($device, $config.config(), $queue, $failed),
            SampleFormat::I32 => $builder::<i32>($device, $config.config(), $queue, $failed),
            SampleFormat::U16 => $builder::<u16>($device, $config.config(), $queue, $failed),
            SampleFormat::U32 => $builder::<u32>($device, $config.config(), $queue, $failed),
            _ => return Err("Format audio non pris en charge".into()),
        }
        .map_err(|err| err.to_string())
    };
}

fn build_input(
    device: &Device,
    config: &SupportedStreamConfig,
    queue: &Arc<FrameQueue>,
    failed: &Arc<AtomicBool>,
) -> Result<Stream, String> {
    let stream = build_stream!(input_stream, device, config, queue, failed)?;
    stream.play().map_err(|err| err.to_string())?;
    Ok(stream)
}

fn build_output(
    device: &Device,
    config: &SupportedStreamConfig,
    queue: &Arc<FrameQueue>,
    failed: &Arc<AtomicBool>,
) -> Result<Stream, String> {
    let stream = build_stream!(output_stream, device, config, queue, failed)?;
    stream.play().map_err(|err| err.to_string())?;
    Ok(stream)
}

fn input_stream<T>(
    device: &Device,
    config: StreamConfig,
    queue: &Arc<FrameQueue>,
    failed: &Arc<AtomicBool>,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let queue = Arc::clone(queue);
    let failed = Arc::clone(failed);
    let hardware_channels = usize::from(config.channels);
    device.build_input_stream(
        &config,
        move |buffer: &[T], _| {
            for frame in buffer.chunks_exact(hardware_channels) {
                queue.push(|channel| f32::from_sample_(frame[channel]));
            }
        },
        move |_| failed.store(true, Ordering::Release),
        None,
    )
}

fn output_stream<T>(
    device: &Device,
    config: StreamConfig,
    queue: &Arc<FrameQueue>,
    failed: &Arc<AtomicBool>,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    let queue = Arc::clone(queue);
    let failed = Arc::clone(failed);
    let hardware_channels = usize::from(config.channels);
    device.build_output_stream(
        &config,
        move |buffer: &mut [T], _| {
            buffer.fill(T::from_sample_(0.0));
            let mut missing = 0;
            for frame in buffer.chunks_exact_mut(hardware_channels) {
                if !queue.pop(|channel, sample| frame[channel] = T::from_sample_(sample)) {
                    missing += 1;
                }
            }
            queue.underruns.fetch_add(missing, Ordering::Relaxed);
        },
        move |_| failed.store(true, Ordering::Release),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_queue_preserves_channel_alignment_at_capacity_and_wraparound() {
        let queue = FrameQueue::new(3);
        for frame in 0..BUFFER_FRAMES {
            assert!(queue.push(|channel| (frame * 3 + channel) as f32));
        }
        assert!(!queue.push(|_| 0.0));
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 1);
        for frame in 0..BUFFER_FRAMES {
            assert!(queue.pop(|channel, sample| assert_eq!(sample, (frame * 3 + channel) as f32)));
            assert!(queue.push(|channel| (frame * 3 + channel + 1) as f32));
        }
        for frame in 0..BUFFER_FRAMES {
            assert!(
                queue.pop(|channel, sample| assert_eq!(sample, (frame * 3 + channel + 1) as f32))
            );
        }
        assert!(!queue.pop(|_, _| unreachable!()));
    }
}
