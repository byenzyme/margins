//! Diagnostic harness for macOS system-audio process taps.
//!
//! Usage:
//!   cargo run -p margins-capture --example tap_probe --features audio-capture -- --duration 20 --mode current --out /tmp/current.wav
//!   cargo run -p margins-capture --example tap_probe --features audio-capture -- --duration 20 --mode stereo-unmuted --out /tmp/stereo.wav
//!
//! Play known audio (music, a tone, or meeting audio) while this runs. The report
//! prints tap format, callback jitter, drop counts, and captured-vs-wall duration.

#[cfg(all(target_os = "macos", feature = "audio-capture"))]
mod macos {
    use anyhow::{Context, Result};
    use clap::{Parser, ValueEnum};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use ca::aggregate_device_keys as agg_keys;
    use ca::sub_device_keys as sub_keys;
    use cidre::{arc, av, cat, cf, core_audio as ca, ns, os};

    #[derive(Debug, Clone, Copy, ValueEnum)]
    enum ProbeMode {
        /// Match Margins's current path: mono global tap, tap-only aggregate, default mute behavior.
        Current,
        /// Current aggregate shape, but explicitly private + unmuted.
        MonoUnmuted,
        /// Stereo global tap, tap-only aggregate, explicitly private + unmuted.
        StereoUnmuted,
        /// Stereo global tap with cidre example aggregate: default output subdevice, main subdevice, auto-start.
        CidreAggregate,
    }

    #[derive(Parser, Debug)]
    #[command(
        name = "tap_probe",
        about = "Probe macOS Core Audio process-tap behavior"
    )]
    struct Args {
        /// Capture duration in seconds.
        #[arg(long, default_value_t = 10.0)]
        duration: f64,

        /// A/B mode to test.
        #[arg(long, value_enum, default_value_t = ProbeMode::Current)]
        mode: ProbeMode,

        /// Output WAV path for captured tap audio.
        #[arg(long, default_value = "/tmp/margins-tap-probe.wav")]
        out: PathBuf,
    }

    struct Ctx {
        producer: rtrb::Producer<f32>,
        format: arc::R<av::AudioFormat>,
        channels: usize,
        started_at: Instant,
        last_callback_ns: Option<u64>,
        callback_count: u64,
        frames_total: u64,
        samples_total: u64,
        dropped_samples: u64,
        peak: f32,
        min_frames: u32,
        max_frames: u32,
        min_interval_ns: u64,
        max_interval_ns: u64,
    }

    impl Ctx {
        fn new(
            producer: rtrb::Producer<f32>,
            format: arc::R<av::AudioFormat>,
            channels: usize,
        ) -> Self {
            Self {
                producer,
                format,
                channels,
                started_at: Instant::now(),
                last_callback_ns: None,
                callback_count: 0,
                frames_total: 0,
                samples_total: 0,
                dropped_samples: 0,
                peak: 0.0,
                min_frames: u32::MAX,
                max_frames: 0,
                min_interval_ns: u64::MAX,
                max_interval_ns: 0,
            }
        }

        fn note_callback(&mut self, frames: u32) {
            let now_ns = self
                .started_at
                .elapsed()
                .as_nanos()
                .min(u128::from(u64::MAX)) as u64;
            if let Some(last) = self.last_callback_ns {
                let interval = now_ns.saturating_sub(last);
                self.min_interval_ns = self.min_interval_ns.min(interval);
                self.max_interval_ns = self.max_interval_ns.max(interval);
            }
            self.last_callback_ns = Some(now_ns);
            self.callback_count += 1;
            self.frames_total += u64::from(frames);
            self.min_frames = self.min_frames.min(frames);
            self.max_frames = self.max_frames.max(frames);
        }

        fn push_sample(&mut self, sample: f32) {
            self.peak = self.peak.max(sample.abs());
            self.samples_total += 1;
            if self.producer.push(sample).is_err() {
                self.dropped_samples += 1;
            }
        }
    }

    pub fn main() -> Result<()> {
        let args = Args::parse();
        let duration = Duration::from_secs_f64(args.duration.max(0.1));

        let output_before =
            describe_default_output().unwrap_or_else(|e| format!("unavailable: {e}"));
        println!("mode: {:?}", args.mode);
        println!("duration: {:.3}s", duration.as_secs_f64());
        println!("default output before: {output_before}");

        let mut tap_desc = match args.mode {
            ProbeMode::Current | ProbeMode::MonoUnmuted => {
                ca::TapDesc::with_mono_global_tap_excluding_processes(&ns::Array::new())
            }
            ProbeMode::StereoUnmuted | ProbeMode::CidreAggregate => {
                ca::TapDesc::with_stereo_global_tap_excluding_processes(&ns::Array::new())
            }
        };

        if !matches!(args.mode, ProbeMode::Current) {
            tap_desc.set_private(true);
            tap_desc.set_mute_behavior(ca::TapMuteBehavior::Unmuted);
        }

        let tap = tap_desc
            .create_process_tap()
            .context("create process tap")?;
        let asbd = tap.asbd().context("tap ASBD")?;
        let rate = asbd.sample_rate as u32;
        let channels = asbd.channels_per_frame.max(1) as usize;
        let format = av::AudioFormat::with_asbd(&asbd).context("tap audio format")?;

        println!(
            "tap format: rate={}Hz channels={} interleaved={} common={:?}",
            rate,
            channels,
            format.is_interleaved(),
            format.common_format()
        );

        let capacity_secs = duration.as_secs_f64().ceil() as usize + 10;
        let capacity = rate as usize * channels * capacity_secs;
        let (producer, mut consumer) = rtrb::RingBuffer::new(capacity.max(4096));
        let mut ctx = Box::new(Ctx::new(producer, format, channels));

        let agg_device = create_aggregate(args.mode, &tap)?;
        let proc_id = agg_device
            .create_io_proc_id(tap_io_proc, Some(&mut ctx))
            .context("create aggregate IO proc")?;

        let wall_start = Instant::now();
        {
            let _started =
                ca::device_start(agg_device, Some(proc_id)).context("start aggregate device")?;
            let output_during =
                describe_default_output().unwrap_or_else(|e| format!("unavailable: {e}"));
            println!("default output during: {output_during}");
            std::thread::sleep(duration);
        }
        let wall_elapsed = wall_start.elapsed();

        let output_after =
            describe_default_output().unwrap_or_else(|e| format!("unavailable: {e}"));
        println!("default output after: {output_after}");

        let mut samples =
            Vec::with_capacity(ctx.samples_total.saturating_sub(ctx.dropped_samples) as usize);
        while let Ok(sample) = consumer.pop() {
            samples.push(sample);
        }

        write_wav(&args.out, &samples, rate, channels as u16).context("write WAV")?;
        print_report(&args, &ctx, samples.len(), wall_elapsed, rate, channels);
        println!("wrote: {}", args.out.display());

        Ok(())
    }

    fn create_aggregate(mode: ProbeMode, tap: &ca::TapGuard) -> Result<ca::AggregateDevice> {
        let sub_tap = cf::DictionaryOf::with_keys_values(
            &[sub_keys::uid()],
            &[tap.uid().context("tap UID")?.as_type_ref()],
        );

        let desc = match mode {
            ProbeMode::CidreAggregate => {
                let output_device =
                    ca::System::default_output_device().context("default output device")?;
                let output_uid = output_device.uid().context("default output UID")?;
                let sub_device = cf::DictionaryOf::with_keys_values(
                    &[sub_keys::uid()],
                    &[output_uid.as_type_ref()],
                );
                cf::DictionaryOf::with_keys_values(
                    &[
                        agg_keys::is_private(),
                        agg_keys::is_stacked(),
                        agg_keys::tap_auto_start(),
                        agg_keys::name(),
                        agg_keys::main_sub_device(),
                        agg_keys::uid(),
                        agg_keys::sub_device_list(),
                        agg_keys::tap_list(),
                    ],
                    &[
                        cf::Boolean::value_true().as_type_ref(),
                        cf::Boolean::value_false(),
                        cf::Boolean::value_true(),
                        cf::String::from_str("margins-tap-probe").as_ref(),
                        &output_uid,
                        &cf::Uuid::new().to_cf_string(),
                        &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
                        &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
                    ],
                )
            }
            ProbeMode::Current | ProbeMode::MonoUnmuted | ProbeMode::StereoUnmuted => {
                cf::DictionaryOf::with_keys_values(
                    &[
                        agg_keys::is_private(),
                        agg_keys::tap_auto_start(),
                        agg_keys::name(),
                        agg_keys::uid(),
                        agg_keys::tap_list(),
                    ],
                    &[
                        cf::Boolean::value_true().as_type_ref(),
                        cf::Boolean::value_false(),
                        cf::String::from_str("margins-tap-probe").as_ref(),
                        &cf::Uuid::new().to_cf_string(),
                        &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
                    ],
                )
            }
        };

        ca::AggregateDevice::with_desc(&desc)
            .map_err(|e| anyhow::anyhow!("create aggregate device: {e}"))
    }

    extern "C" fn tap_io_proc(
        _device: ca::Device,
        _now: &cat::AudioTimeStamp,
        input_data: &cat::AudioBufList<1>,
        _input_time: &cat::AudioTimeStamp,
        _output_data: &mut cat::AudioBufList<1>,
        _output_time: &cat::AudioTimeStamp,
        ctx: Option<&mut Ctx>,
    ) -> os::Status {
        let Some(ctx) = ctx else {
            return os::Status::NO_ERR;
        };

        if let Some(view) = av::AudioPcmBuf::with_buf_list_no_copy(&ctx.format, input_data, None) {
            let frames = view.frame_len();
            ctx.note_callback(frames);
            let channels = ctx.channels.max(1);
            let frame_len = frames as usize;
            let stride = view.stride();

            if let Some(data) = view.data_f32_at(0) {
                if stride > 1 || channels == 1 {
                    for &sample in data {
                        ctx.push_sample(sample);
                    }
                    return os::Status::NO_ERR;
                }
            }

            // Non-interleaved stereo/multichannel: write interleaved frames to the probe WAV.
            for frame in 0..frame_len {
                for ch in 0..channels {
                    let sample = view
                        .data_f32_at(ch)
                        .and_then(|channel| channel.get(frame).copied())
                        .unwrap_or(0.0);
                    ctx.push_sample(sample);
                }
            }
            return os::Status::NO_ERR;
        }

        // Raw fallback: assume the first AudioBuffer contains f32 samples.
        let buf = &input_data.buffers[0];
        if buf.data_bytes_size > 0 && !buf.data.is_null() {
            let count = buf.data_bytes_size as usize / std::mem::size_of::<f32>();
            if count > 0 {
                ctx.note_callback((count / ctx.channels.max(1)) as u32);
                let data = unsafe { std::slice::from_raw_parts(buf.data as *const f32, count) };
                for &sample in data {
                    ctx.push_sample(sample);
                }
            }
        }

        os::Status::NO_ERR
    }

    fn write_wav(path: &PathBuf, samples: &[f32], sample_rate: u32, channels: u16) -> Result<()> {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec)
            .with_context(|| format!("create {}", path.display()))?;
        for &sample in samples {
            writer.write_sample(sample.clamp(-1.0, 1.0))?;
        }
        writer.finalize()?;
        Ok(())
    }

    fn print_report(
        args: &Args,
        ctx: &Ctx,
        written_samples: usize,
        wall: Duration,
        rate: u32,
        channels: usize,
    ) {
        let captured_secs = written_samples as f64 / channels.max(1) as f64 / rate as f64;
        let expected_interval_ms = if ctx.callback_count > 0 {
            (ctx.frames_total as f64 / ctx.callback_count as f64) / rate as f64 * 1000.0
        } else {
            0.0
        };
        let min_interval_ms = if ctx.min_interval_ns == u64::MAX {
            0.0
        } else {
            ctx.min_interval_ns as f64 / 1_000_000.0
        };
        let max_interval_ms = ctx.max_interval_ns as f64 / 1_000_000.0;
        let min_frames = if ctx.min_frames == u32::MAX {
            0
        } else {
            ctx.min_frames
        };

        println!("\n=== tap probe report ===");
        println!("mode: {:?}", args.mode);
        println!("wall duration: {:.3}s", wall.as_secs_f64());
        println!("captured duration: {:.3}s", captured_secs);
        println!(
            "duration delta: {:+.3}s",
            captured_secs - wall.as_secs_f64()
        );
        println!("callbacks: {}", ctx.callback_count);
        println!(
            "callback frames: min={} max={} avg={:.1}",
            min_frames,
            ctx.max_frames,
            avg(ctx.frames_total, ctx.callback_count)
        );
        println!(
            "callback interval: min={:.3}ms max={:.3}ms expected≈{:.3}ms",
            min_interval_ms, max_interval_ms, expected_interval_ms
        );
        println!("samples observed: {}", ctx.samples_total);
        println!("samples written: {}", written_samples);
        println!("dropped samples: {}", ctx.dropped_samples);
        println!("peak: {:.6} ({:.1} dBFS)", ctx.peak, dbfs(ctx.peak));
        if ctx.peak <= 1e-7 {
            println!(
                "warning: tap delivered exact silence. If audio was playing, check Screen & System Audio Recording permission for the launching app/process."
            );
        }
    }

    fn avg(total: u64, count: u64) -> f64 {
        if count == 0 {
            0.0
        } else {
            total as f64 / count as f64
        }
    }

    fn dbfs(v: f32) -> f32 {
        if v > 0.0 {
            20.0 * v.log10()
        } else {
            -99.0
        }
    }

    fn describe_default_output() -> Result<String> {
        let device = ca::System::default_output_device().context("default output device")?;
        let name = device
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "<unknown>".into());
        let uid = device
            .uid()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "<unknown>".into());
        Ok(format!("{name} ({uid})"))
    }
}

#[cfg(all(target_os = "macos", feature = "audio-capture"))]
fn main() -> anyhow::Result<()> {
    macos::main()
}

#[cfg(all(target_os = "macos", not(feature = "audio-capture")))]
fn main() {
    eprintln!("tap_probe requires the audio-capture feature");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("tap_probe is only available on macOS");
}
