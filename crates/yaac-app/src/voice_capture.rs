//! The microphone for voice input on the desktop (docs/plan/voice.md), for the renderers that
//! do not record themselves (egui, WPF): the controller opens the default input device with
//! `cpal` on its own thread, never the UI thread, and hands out `pcm16le` chunks in the
//! harness's format. Only on Windows (WASAPI) with the `desktop` feature: elsewhere
//! [`AVAILABLE`] is false and the controller leaves recording to the renderer (Android) or
//! offers no microphone (egui on Linux).

use std::sync::mpsc;

/// What the capture thread reports, in order.
#[derive(Debug)]
#[cfg_attr(not(all(windows, feature = "desktop")), allow(dead_code))]
pub(crate) enum Captured {
    /// One chunk of audio in the harness's format.
    Audio(Vec<u8>),
    /// The input's loudness (0..1) for the indicator.
    Level(f32),
    /// Recording stopped by itself; words for the user.
    Failed(String),
    /// After [`Capture::stop`]: the last audio has been handed out.
    Done,
}

/// Whether this build can record in the controller.
pub(crate) const AVAILABLE: bool = cfg!(all(windows, feature = "desktop"));

/// A running recording. Dropping it cancels.
pub(crate) struct Capture {
    #[cfg_attr(not(all(windows, feature = "desktop")), allow(dead_code))]
    control: mpsc::Sender<bool>,
    pub(crate) events: mpsc::Receiver<Captured>,
}

impl Capture {
    /// Stop and hand out what is left, then [`Captured::Done`].
    pub(crate) fn stop(&self) {
        let _ = self.control.send(true);
    }

    /// Stop and drop what is left.
    pub(crate) fn cancel(&self) {
        let _ = self.control.send(false);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.control.send(false);
    }
}

/// Start recording for `rate` Hz, `channels` channels. `wake` is called (from the capture
/// thread) whenever there is something in [`Capture::events`].
#[cfg(all(windows, feature = "desktop"))]
pub(crate) fn start(rate: u32, channels: u16, wake: impl Fn() + Send + 'static) -> Result<Capture, String> {
    let (control, stop) = mpsc::channel::<bool>();
    let (tx, events) = mpsc::channel();
    let (ready_tx, ready) = mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("yaac-voice".into())
        .spawn(move || run(rate, channels, &stop, &tx, &wake, &ready_tx))
        .map_err(|e| format!("The microphone could not start: {e}"))?;
    match ready.recv() {
        Ok(Ok(())) => Ok(Capture { control, events }),
        Ok(Err(why)) => Err(why),
        Err(_) => Err("The microphone could not start.".into()),
    }
}

#[cfg(not(all(windows, feature = "desktop")))]
pub(crate) fn start(_rate: u32, _channels: u16, _wake: impl Fn() + Send + 'static) -> Result<Capture, String> {
    Err("This build cannot record the microphone.".into())
}

#[cfg(all(windows, feature = "desktop"))]
fn run(
    rate: u32,
    channels: u16,
    control: &mpsc::Receiver<bool>,
    tx: &mpsc::Sender<Captured>,
    wake: &(impl Fn() + Send + 'static),
    ready: &mpsc::Sender<Result<(), String>>,
) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::SampleFormat;
    use std::time::Duration;

    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        let _ = ready.send(Err("No microphone was found.".into()));
        return;
    };
    let config = match device.default_input_config() {
        Ok(config) => config,
        Err(e) => {
            let _ = ready.send(Err(format!("The microphone could not open: {e}")));
            return;
        }
    };
    let device_rate = config.sample_rate();
    let device_channels = config.channels();
    // The device's callback only converts to f32 and passes samples on; resampling and
    // chunking happen on this thread.
    let (samples_tx, samples) = mpsc::sync_channel::<Vec<f32>>(64);
    let failed = tx.clone();
    let on_error = move |e: cpal::StreamError| {
        let _ = failed.send(Captured::Failed(format!("The microphone stopped: {e}")));
    };
    let stream_config: cpal::StreamConfig = config.config();
    macro_rules! input {
        ($t:ty) => {{
            let samples_tx = samples_tx.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let _ = samples_tx.try_send(data.iter().map(|s| cpal::Sample::to_sample::<f32>(*s)).collect());
                },
                on_error,
                None,
            )
        }};
    }
    let stream = match config.sample_format() {
        SampleFormat::F32 => input!(f32),
        SampleFormat::I16 => input!(i16),
        SampleFormat::U16 => input!(u16),
        SampleFormat::I32 => input!(i32),
        other => {
            let _ = ready.send(Err(format!("The microphone's sample format ({other}) is not supported.")));
            return;
        }
    };
    let stream = match stream {
        Ok(stream) => stream,
        Err(e) => {
            let _ = ready.send(Err(format!("The microphone could not open: {e}")));
            return;
        }
    };
    if let Err(e) = stream.play() {
        let _ = ready.send(Err(format!("The microphone could not start: {e}")));
        return;
    }
    let _ = ready.send(Ok(()));
    let mut pipeline = crate::voice_audio::Pipeline::new(device_rate, device_channels, rate, channels);
    let send = |chunks: Vec<Vec<u8>>, level: f32| {
        for chunk in chunks {
            let _ = tx.send(Captured::Audio(chunk));
        }
        let _ = tx.send(Captured::Level(level));
        wake();
    };
    let keep = loop {
        match control.try_recv() {
            Ok(keep) => break keep,
            Err(mpsc::TryRecvError::Disconnected) => break false,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match samples.recv_timeout(Duration::from_millis(50)) {
            Ok(data) => {
                let (chunks, level) = pipeline.push(&data);
                send(chunks, level);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
        }
    };
    drop(stream);
    if keep {
        while let Ok(data) = samples.try_recv() {
            let (chunks, level) = pipeline.push(&data);
            send(chunks, level);
        }
        if let Some(rest) = pipeline.flush() {
            let _ = tx.send(Captured::Audio(rest));
        }
        let _ = tx.send(Captured::Done);
        wake();
    }
}
