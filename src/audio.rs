use anyhow::{Context, Result, bail};
use cpal::{
    FromSample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::{
    fs::File,
    io::BufWriter,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

type Writer = Arc<Mutex<Option<hound::WavWriter<BufWriter<File>>>>>;
pub struct Capture {
    stream: cpal::Stream,
    writer: Writer,
    pub level: Arc<AtomicU32>,
    pub error: Arc<Mutex<Option<String>>>,
    pub device: String,
}
pub fn devices() -> Result<Vec<String>> {
    cpal::default_host()
        .input_devices()?
        .map(|d| {
            d.description()
                .map(|d| d.name().to_owned())
                .map_err(Into::into)
        })
        .collect()
}
impl Capture {
    pub fn start(path: &Path, name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = if let Some(name) = name {
            host.input_devices()?
                .find(|d| d.description().is_ok_and(|d| d.name() == name))
        } else {
            host.default_input_device()
        }
        .context("Microphone not found. Run `pasteface devices` and set AUDIO_DEVICE.")?;
        let device_name = device.description()?.name().to_owned();
        let supported = device.default_input_config()?;
        let config: cpal::StreamConfig = supported.into();
        let writer = Arc::new(Mutex::new(Some(hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: config.sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?)));
        let level = Arc::new(AtomicU32::new(0));
        let error = Arc::new(Mutex::new(None));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, &writer, &level, &error),
            SampleFormat::I16 => build::<i16>(&device, &config, &writer, &level, &error),
            SampleFormat::U16 => build::<u16>(&device, &config, &writer, &level, &error),
            SampleFormat::I32 => build::<i32>(&device, &config, &writer, &level, &error),
            SampleFormat::F64 => build::<f64>(&device, &config, &writer, &level, &error),
            format => bail!("Unsupported microphone sample format: {format:?}"),
        }?;
        stream.play()?;
        Ok(Self {
            stream,
            writer,
            level,
            error,
            device: device_name,
        })
    }
    pub fn stop(self) -> Result<()> {
        drop(self.stream);
        if let Some(writer) = self.writer.lock().unwrap().take() {
            writer.finalize()?;
        }
        if let Some(error) = self.error.lock().unwrap().take() {
            bail!(error);
        }
        Ok(())
    }
}
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    writer: &Writer,
    level: &Arc<AtomicU32>,
    error: &Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let writer = writer.clone();
    let level = level.clone();
    let write_error = error.clone();
    let stream_error = error.clone();
    let channels = config.channels as usize;
    Ok(device.build_input_stream(
        *config,
        move |data: &[T], _| {
            let mut guard = writer.lock().unwrap();
            let Some(writer) = guard.as_mut() else {
                return;
            };
            let mut peak = 0.0_f32;
            for frame in data.chunks_exact(channels) {
                let mono =
                    frame.iter().map(|v| v.to_sample::<f32>()).sum::<f32>() / channels as f32;
                peak = peak.max(mono.abs());
                if let Err(e) = writer.write_sample((mono.clamp(-1., 1.) * i16::MAX as f32) as i16)
                {
                    *write_error.lock().unwrap() = Some(e.to_string());
                    break;
                }
            }
            level.store(peak.to_bits(), Ordering::Relaxed);
        },
        move |e| {
            record_stream_error(&stream_error, e);
        },
        None,
    )?)
}

/// Recorded length comes from sample frames, independent of device channel count.
pub fn duration(path: &Path) -> Result<f64> {
    let wav = hound::WavReader::open(path)?;
    Ok(f64::from(wav.duration()) / f64::from(wav.spec().sample_rate))
}

// CPAL recovers xruns by preparing and restarting capture. A later recovery
// failure is delivered separately and must still stop capture.
fn record_stream_error(error: &Arc<Mutex<Option<String>>>, event: cpal::Error) {
    if event.kind() != cpal::ErrorKind::Xrun {
        *error.lock().unwrap() = Some(event.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recoverable_overrun_does_not_abort_capture_but_disconnect_does() {
        let error = Arc::new(Mutex::new(None));
        record_stream_error(&error, cpal::ErrorKind::Xrun.into());
        assert!(error.lock().unwrap().is_none());
        record_stream_error(&error, cpal::ErrorKind::DeviceNotAvailable.into());
        let fatal = error.lock().unwrap().clone();
        assert!(fatal.is_some());
        record_stream_error(&error, cpal::ErrorKind::Xrun.into());
        assert_eq!(*error.lock().unwrap(), fatal);
    }
}
