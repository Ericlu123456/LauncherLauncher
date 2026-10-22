//! A tiny procedural "keygen" chiptune player.
//!
//! The tune is synthesised at startup (no audio files, no assets), then looped
//! through CPAL. Muting is a volume flag, so the loop keeps its position.

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub struct AudioEngine {
    _stream: cpal::Stream,
    playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    base_volume: f32,
}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("没有默认音频输出设备")?;
        let supported = device.default_output_config()?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        // Render at the device's actual rate; otherwise 48 kHz devices play
        // the tune ~9% fast and a semitone high.
        let samples = Arc::new(render_tune(config.sample_rate.0));
        let playing = Arc::new(AtomicBool::new(true));
        let volume = Arc::new(AtomicU32::new(0.35f32.to_bits()));

        let stream = match sample_format {
            SampleFormat::F32 => {
                build_stream::<f32>(&device, &config, samples, playing.clone(), volume.clone())?
            }
            SampleFormat::I16 => {
                build_stream::<i16>(&device, &config, samples, playing.clone(), volume.clone())?
            }
            SampleFormat::U16 => {
                build_stream::<u16>(&device, &config, samples, playing.clone(), volume.clone())?
            }
            other => return Err(anyhow!("不支持的音频采样格式: {other:?}")),
        };
        stream.play()?;

        Ok(Self {
            _stream: stream,
            playing,
            volume,
            base_volume: 0.35,
        })
    }

    pub fn set_playing(&self, on: bool) {
        self.playing.store(on, Ordering::Relaxed);
    }

    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        let v = if muted { 0.0 } else { self.base_volume };
        self.volume.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn muted(&self) -> bool {
        f32::from_bits(self.volume.load(Ordering::Relaxed)) == 0.0
    }

    pub fn set_volume(&mut self, v: f32) {
        self.base_volume = v.clamp(0.0, 1.0);
        if !self.muted() {
            self.volume
                .store(self.base_volume.to_bits(), Ordering::Relaxed);
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: Arc<Vec<f32>>,
    playing: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut pos = 0usize;
    let err_fn = |e| eprintln!("audio stream error: {e}");
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let play = playing.load(Ordering::Relaxed);
            let vol = f32::from_bits(volume.load(Ordering::Relaxed));
            for frame in data.chunks_mut(channels) {
                let s = if play { samples[pos] * vol } else { 0.0 };
                for out in frame.iter_mut() {
                    *out = T::from_sample(s);
                }
                pos += 1;
                if pos >= samples.len() {
                    pos = 0;
                }
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

#[derive(Clone, Copy)]
enum Wave {
    Square,
    Triangle,
}

fn midi_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

fn envelope(t: f32, dur: f32) -> f32 {
    let attack = 0.006;
    let release = 0.05;
    let a = (t / attack).min(1.0);
    let r = ((dur - t) / release).clamp(0.0, 1.0);
    a * r
}

fn add_note(buf: &mut [f32], sr: f32, start_s: f32, dur_s: f32, freq: f32, amp: f32, wave: Wave) {
    let start = (start_s * sr) as usize;
    let n = (dur_s * sr) as usize;
    if start >= buf.len() {
        return;
    }
    for i in 0..n {
        let idx = start + i;
        if idx >= buf.len() {
            break;
        }
        let t = i as f32 / sr;
        let env = envelope(t, dur_s);
        let ph = TAU * freq * t;
        let v = match wave {
            Wave::Square => {
                let a = if ph.sin() >= 0.0 { 1.0 } else { -1.0 };
                let b = if (ph * 1.006).sin() >= 0.0 { 1.0 } else { -1.0 };
                (a + b) * 0.5
            }
            Wave::Triangle => {
                let x = (ph / TAU).fract();
                4.0 * (x - 0.5).abs() - 1.0
            }
        };
        buf[idx] += v * amp * env;
    }
}

fn add_kick(buf: &mut [f32], sr: f32, start_s: f32) {
    let start = (start_s * sr) as usize;
    let n = (0.18 * sr) as usize;
    let mut phase = 0.0f32;
    for i in 0..n {
        let idx = start + i;
        if idx >= buf.len() {
            break;
        }
        let t = i as f32 / sr;
        let f = 140.0 * (-t * 20.0).exp() + 45.0;
        phase += TAU * f / sr;
        let amp = (-t * 9.0).exp() * 0.55;
        buf[idx] += phase.sin() * amp;
    }
}

fn add_hat(buf: &mut [f32], sr: f32, start_s: f32, seed: &mut u32) {
    let start = (start_s * sr) as usize;
    let n = (0.045 * sr) as usize;
    let mut prev = 0.0f32;
    for i in 0..n {
        let idx = start + i;
        if idx >= buf.len() {
            break;
        }
        // xorshift noise, then a crude high-pass.
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        let white = (*seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let hp = white - prev;
        prev = white;
        let t = i as f32 / sr;
        let amp = (-t * 80.0).exp() * 0.08;
        buf[idx] += hp * amp;
    }
}

fn render_tune(sr: u32) -> Vec<f32> {
    let sr_f = sr as f32;
    let bpm = 128.0f32;
    let beat = 60.0 / bpm;
    let bars = 8usize;
    let total = (bars as f32 * 4.0 * beat * sr_f) as usize;
    let mut buf = vec![0.0f32; total];

    // Am – F – C – G, two bars each.
    let chords: [(i32, [i32; 4]); 4] = [
        (45, [0, 3, 7, 12]), // A minor
        (41, [0, 4, 7, 12]), // F major
        (48, [0, 4, 7, 12]), // C major
        (43, [0, 4, 7, 12]), // G major
    ];

    for bar in 0..bars {
        let (root, intervals) = chords[bar % 4];
        let bar_start = bar as f32 * 4.0 * beat;

        for b in 0..4 {
            add_note(
                &mut buf,
                sr_f,
                bar_start + b as f32 * beat,
                beat * 0.92,
                midi_hz((root - 12) as f32),
                0.20,
                Wave::Triangle,
            );
        }

        for step in 0..16 {
            let t = bar_start + step as f32 * beat / 4.0;
            let iv = intervals[step % intervals.len()];
            let octave = if (step / 4) % 2 == 0 { 12 } else { 24 };
            add_note(
                &mut buf,
                sr_f,
                t,
                beat / 4.0 * 0.85,
                midi_hz((root + iv + octave) as f32),
                0.085,
                Wave::Square,
            );
            if step % 4 == 2 {
                add_note(
                    &mut buf,
                    sr_f,
                    t + beat / 8.0,
                    beat / 4.0 * 0.8,
                    midi_hz((root + iv + octave + 12) as f32),
                    0.045,
                    Wave::Square,
                );
            }
        }

        add_kick(&mut buf, sr_f, bar_start);
        add_kick(&mut buf, sr_f, bar_start + 2.0 * beat);

        let mut seed = 0x1234_5678u32 ^ (bar as u32).wrapping_mul(2654435761);
        for h in 0..8 {
            add_hat(&mut buf, sr_f, bar_start + h as f32 * beat / 2.0, &mut seed);
        }
    }

    // Dotted-eighth echo for that unmistakable keygen sparkle.
    let delay = (beat * 0.75 * sr_f) as usize;
    let dry = buf.clone();
    for i in delay..buf.len() {
        buf[i] += dry[i - delay] * 0.22;
    }

    let peak = buf.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    if peak > 0.0 {
        let g = 0.72 / peak;
        for s in &mut buf {
            *s *= g;
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tune_is_sane() {
        let s = render_tune(44_100);
        assert_eq!(s.len(), (8.0 * 4.0 * (60.0 / 128.0) * 44_100.0) as usize);
        let peak = s.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.5 && peak <= 0.73, "peak={peak}");
        assert!(s.iter().all(|v| v.is_finite()));
    }
}
