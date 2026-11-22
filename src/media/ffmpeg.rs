//! FFmpeg subprocess pipes: raw rgb24 decode / encode, single-frame extraction, mux args.

use image::RgbImage;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, ChildStdout, Stdio};

use super::Rational;
use super::encoders::{Codec, VideoEncoder};
use crate::config::{Container, Priority};
use crate::pipeline::cancel::CancelToken;
use crate::util::{ChildGuard, ProcError, StderrTail, new_command, run_capture, spawn};

/// Arguments for a raw-video decode.
#[derive(Clone, Debug)]
pub struct DecodeSpec {
    pub input: PathBuf,
    /// (start seconds, duration seconds)
    pub segment: Option<(f64, f64)>,
    /// Output is forced to this constant frame rate (VFR → CFR conversion).
    pub fps: Option<Rational>,
    pub width: u32,
    pub height: u32,
    /// Scale to `width`x`height` (otherwise the source size must equal it).
    pub scale: bool,
}

/// A running `ffmpeg … -f rawvideo -pix_fmt rgb24 -` process.
pub struct Decoder {
    guard: ChildGuard,
    stdout: ChildStdout,
    tail: Option<StderrTail>,
    width: u32,
    height: u32,
}

impl Decoder {
    pub fn spawn(
        ffmpeg: &Path,
        spec: &DecodeSpec,
        priority: Priority,
    ) -> Result<Decoder, ProcError> {
        let mut cmd = new_command(ffmpeg);
        cmd.args(["-hide_banner", "-nostdin", "-v", "error"]);
        if let Some((start, _)) = spec.segment
            && start > 0.0
        {
            cmd.args(["-ss", &format!("{start:.3}")]);
        }
        cmd.arg("-i").arg(&spec.input);
        if let Some((_, dur)) = spec.segment {
            cmd.args(["-t", &format!("{dur:.3}")]);
        }
        let mut filters = Vec::new();
        if let Some(fps) = spec.fps {
            filters.push(format!("fps={}", fps.ffmpeg()));
        }
        if spec.scale {
            filters.push(format!(
                "scale={}:{}:flags=bicubic",
                spec.width, spec.height
            ));
        }
        cmd.args(["-map", "0:v:0", "-an", "-sn", "-dn"]);
        if !filters.is_empty() {
            cmd.args(["-vf", &filters.join(",")]);
        }
        cmd.args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = spawn(&mut cmd, priority)?;
        let stdout = child.stdout.take().expect("stdout piped");
        let tail = StderrTail::spawn(child.stderr.take().expect("stderr piped"), 30);
        Ok(Decoder {
            guard: ChildGuard(Some(child)),
            stdout,
            tail: Some(tail),
            width: spec.width,
            height: spec.height,
        })
    }

    /// Read the next frame; `Ok(None)` at end of stream.
    pub fn read_frame(&mut self) -> std::io::Result<Option<RgbImage>> {
        let len = self.width as usize * self.height as usize * 3;
        let mut buf = vec![0u8; len];
        let mut filled = 0;
        while filled < len {
            match self.stdout.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        if filled == 0 {
            return Ok(None);
        }
        if filled < len {
            // Truncated last frame: treat as end of stream.
            return Ok(None);
        }
        Ok(RgbImage::from_raw(self.width, self.height, buf))
    }

    /// Wait for exit and report failure with the stderr tail.
    pub fn finish(mut self) -> Result<(), ProcError> {
        let mut child = self.guard.0.take().expect("child");
        drop(self.stdout);
        let status = child.wait()?;
        let tail = self.tail.take().map(|t| t.finish()).unwrap_or_default();
        if status.success() {
            Ok(())
        } else {
            Err(ProcError::Failed {
                program: "ffmpeg (decode)".into(),
                code: format!("{status}"),
                tail,
            })
        }
    }

    pub fn stderr_tail(&self) -> String {
        self.tail.as_ref().map(|t| t.text()).unwrap_or_default()
    }
}

/// Where to take audio / subtitles / metadata from when encoding.
#[derive(Clone, Debug)]
pub struct MuxSource {
    pub path: PathBuf,
    pub segment: Option<(f64, f64)>,
    pub audio_codecs: Vec<String>,
    pub subtitle_codecs: Vec<String>,
    pub include_subtitles: bool,
}

#[derive(Clone, Debug)]
pub struct EncodeSpec {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub encoder: VideoEncoder,
    pub quality: Option<u32>,
    pub container: Container,
    pub output: PathBuf,
    pub mux: Option<MuxSource>,
}

const MP4_AUDIO_COPY: [&str; 7] = ["aac", "mp3", "ac3", "eac3", "opus", "alac", "flac"];
const TEXT_SUBS: [&str; 6] = ["subrip", "srt", "ass", "ssa", "mov_text", "webvtt"];

/// Audio / subtitle / metadata mapping arguments (input #1 is the source file).
pub fn mux_args(container: Container, mux: &MuxSource) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    if !mux.audio_codecs.is_empty() {
        a.extend(s(&["-map", "1:a?"]));
        let copy = match container {
            Container::Mkv => true,
            Container::Mp4 => mux
                .audio_codecs
                .iter()
                .all(|c| MP4_AUDIO_COPY.contains(&c.as_str())),
        };
        if copy {
            a.extend(s(&["-c:a", "copy"]));
        } else {
            a.extend(s(&["-c:a", "aac", "-b:a", "192k"]));
        }
    }
    if mux.include_subtitles && !mux.subtitle_codecs.is_empty() {
        match container {
            Container::Mkv => a.extend(s(&["-map", "1:s?", "-c:s", "copy"])),
            Container::Mp4 => {
                if mux
                    .subtitle_codecs
                    .iter()
                    .all(|c| TEXT_SUBS.contains(&c.as_str()))
                {
                    a.extend(s(&["-map", "1:s?", "-c:s", "mov_text"]));
                } else {
                    tracing::warn!(
                        "bitmap subtitles ({}) can't go into MP4; dropped (use MKV to keep them)",
                        mux.subtitle_codecs.join(",")
                    );
                }
            }
        }
    }
    a.extend(s(&["-map_metadata", "1", "-map_chapters", "1"]));
    a
}

/// A running encoder fed with raw rgb24 frames on stdin.
pub struct Encoder {
    guard: ChildGuard,
    stdin: Option<ChildStdin>,
    tail: Option<StderrTail>,
    width: u32,
    height: u32,
    pub frames_written: u64,
}

impl Encoder {
    pub fn build_args(spec: &EncodeSpec) -> Vec<String> {
        let mut a: Vec<String> = [
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        a.push("-s".into());
        a.push(format!("{}x{}", spec.width, spec.height));
        a.push("-framerate".into());
        a.push(spec.fps.ffmpeg());
        a.push("-i".into());
        a.push("-".into());
        if let Some(m) = &spec.mux {
            if let Some((start, dur)) = m.segment {
                if start > 0.0 {
                    a.push("-ss".into());
                    a.push(format!("{start:.3}"));
                }
                a.push("-t".into());
                a.push(format!("{dur:.3}"));
            }
            a.push("-i".into());
            a.push(m.path.to_string_lossy().to_string());
        }
        a.push("-map".into());
        a.push("0:v:0".into());
        if let Some(m) = &spec.mux {
            a.extend(mux_args(spec.container, m));
        }
        a.extend(spec.encoder.args(spec.quality));
        if spec.container == Container::Mp4 {
            if spec.encoder.codec() == Codec::Hevc {
                a.push("-tag:v".into());
                a.push("hvc1".into());
            }
            a.push("-movflags".into());
            a.push("+faststart".into());
        }
        a.push("-f".into());
        a.push(spec.container.ffmpeg_format().into());
        a.push(spec.output.to_string_lossy().to_string());
        a
    }

    pub fn spawn(
        ffmpeg: &Path,
        spec: &EncodeSpec,
        priority: Priority,
    ) -> Result<Encoder, ProcError> {
        let mut cmd = new_command(ffmpeg);
        cmd.args(Self::build_args(spec));
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = spawn(&mut cmd, priority)?;
        let stdin = child.stdin.take();
        let tail = StderrTail::spawn(child.stderr.take().expect("stderr piped"), 30);
        Ok(Encoder {
            guard: ChildGuard(Some(child)),
            stdin,
            tail: Some(tail),
            width: spec.width,
            height: spec.height,
            frames_written: 0,
        })
    }

    pub fn write_frame(&mut self, img: &RgbImage) -> std::io::Result<()> {
        debug_assert_eq!((img.width(), img.height()), (self.width, self.height));
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("encoder stdin closed"))?;
        stdin.write_all(img.as_raw())?;
        self.frames_written += 1;
        Ok(())
    }

    /// Close stdin and wait for ffmpeg to finalize the file.
    pub fn finish(mut self) -> Result<(), ProcError> {
        drop(self.stdin.take());
        let mut child = self.guard.0.take().expect("child");
        let status = child.wait()?;
        let tail = self.tail.take().map(|t| t.finish()).unwrap_or_default();
        if status.success() {
            Ok(())
        } else {
            Err(ProcError::Failed {
                program: "ffmpeg (encode)".into(),
                code: format!("{status}"),
                tail,
            })
        }
    }

    /// Kill the encoder (partial output stays on disk; caller decides what to do with it).
    pub fn abort(mut self) -> String {
        drop(self.stdin.take());
        if let Some(mut c) = self.guard.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.tail.take().map(|t| t.finish()).unwrap_or_default()
    }
}

/// Extract a single frame at `time` seconds, scaled to `w`x`h` if `scale`.
pub fn extract_frame(
    ffmpeg: &Path,
    input: &Path,
    time: f64,
    w: u32,
    h: u32,
    scale: bool,
    cancel: Option<&CancelToken>,
) -> Result<RgbImage, ProcError> {
    let mut cmd = new_command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-v", "error"]);
    if time > 0.0 {
        cmd.args(["-ss", &format!("{time:.3}")]);
    }
    cmd.arg("-i").arg(input);
    cmd.args(["-map", "0:v:0", "-frames:v", "1"]);
    if scale {
        cmd.args(["-vf", &format!("scale={w}:{h}:flags=bicubic")]);
    }
    cmd.args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
    let out = run_capture(cmd, cancel, Priority::Normal)?;
    let len = w as usize * h as usize * 3;
    if out.stdout.len() < len {
        return Err(ProcError::Failed {
            program: "ffmpeg (frame)".into(),
            code: format!("got {} of {} bytes", out.stdout.len(), len),
            tail: out.stderr_tail,
        });
    }
    let mut v = out.stdout;
    v.truncate(len);
    Ok(RgbImage::from_raw(w, h, v).expect("size checked"))
}

/// Decode a whole (short) clip into memory at a given size. Used by the sample player.
/// Decoded PCM audio (interleaved f32).
#[derive(Clone, Debug)]
pub struct AudioClip {
    pub samples: std::sync::Arc<[f32]>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioClip {
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / self.channels.max(1) as f64 / self.sample_rate.max(1) as f64
    }
}

/// Decode the audio of `input` (optionally a `(start, duration)` segment) to stereo 48 kHz
/// f32 PCM through a pipe. `Ok(None)` when the file has no audio stream.
pub fn decode_audio(
    ffmpeg: &Path,
    input: &Path,
    segment: Option<(f64, f64)>,
    cancel: &CancelToken,
) -> Result<Option<AudioClip>, ProcError> {
    const RATE: u32 = 48_000;
    const CH: u16 = 2;
    let mut cmd = new_command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-v", "error"]);
    if let Some((start, dur)) = segment {
        cmd.args(["-ss", &format!("{start:.3}"), "-t", &format!("{dur:.3}")]);
    }
    cmd.arg("-i").arg(input);
    cmd.args([
        "-map", "0:a:0?", "-vn", "-sn", "-dn", "-ac", &CH.to_string(), "-ar", &RATE.to_string(),
        "-f", "f32le", "-",
    ]);
    let out = match run_capture(cmd, Some(cancel), Priority::Normal) {
        Ok(o) => o,
        // `0:a:0?` matched nothing: ffmpeg refuses to write an output without streams.
        Err(ProcError::Failed { tail, .. }) if tail.contains("does not contain any stream") => {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    if out.stdout.len() < 4 {
        return Ok(None);
    }
    let samples: Vec<f32> = out
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    Ok(Some(AudioClip {
        samples: samples.into(),
        sample_rate: RATE,
        channels: CH,
    }))
}

pub fn decode_clip(
    ffmpeg: &Path,
    spec: &DecodeSpec,
    max_frames: usize,
    cancel: &CancelToken,
) -> Result<Vec<RgbImage>, ProcError> {
    let mut dec = Decoder::spawn(ffmpeg, spec, Priority::Normal)?;
    let mut frames = Vec::new();
    while frames.len() < max_frames {
        if cancel.is_cancelled() {
            return Err(ProcError::Cancelled);
        }
        match dec.read_frame()? {
            Some(f) => frames.push(f),
            None => break,
        }
    }
    if frames.len() >= max_frames {
        // Enough; the process is killed by the guard on drop.
        return Ok(frames);
    }
    let tail = dec.stderr_tail();
    dec.finish().map_err(|e| match e {
        ProcError::Failed { program, code, .. } => ProcError::Failed {
            program,
            code,
            tail,
        },
        e => e,
    })?;
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(audio: &[&str], subs: &[&str]) -> MuxSource {
        MuxSource {
            path: "in.mkv".into(),
            segment: None,
            audio_codecs: audio.iter().map(|s| s.to_string()).collect(),
            subtitle_codecs: subs.iter().map(|s| s.to_string()).collect(),
            include_subtitles: true,
        }
    }

    #[test]
    fn mux_mp4_reencodes_unfriendly_audio_and_converts_text_subs() {
        let a = mux_args(Container::Mp4, &src(&["pcm_s16le"], &["subrip"]));
        let j = a.join(" ");
        assert!(j.contains("-c:a aac"));
        assert!(j.contains("-c:s mov_text"));
        let a = mux_args(Container::Mp4, &src(&["aac"], &["hdmv_pgs_subtitle"])).join(" ");
        assert!(a.contains("-c:a copy"));
        assert!(!a.contains("-c:s"));
    }

    #[test]
    fn mux_mkv_copies_everything() {
        let a = mux_args(Container::Mkv, &src(&["pcm_s16le"], &["hdmv_pgs_subtitle"])).join(" ");
        assert!(a.contains("-c:a copy"));
        assert!(a.contains("-c:s copy"));
    }

    #[test]
    fn encoder_args_shape() {
        let spec = EncodeSpec {
            width: 1280,
            height: 720,
            fps: Rational::new(60000, 1001),
            encoder: VideoEncoder::HevcNvenc,
            quality: None,
            container: Container::Mp4,
            output: "out.mp4".into(),
            mux: Some(src(&["aac"], &[])),
        };
        let a = Encoder::build_args(&spec).join(" ");
        assert!(a.contains("-s 1280x720"));
        assert!(a.contains("-framerate 60000/1001"));
        assert!(a.contains("-tag:v hvc1"));
        assert!(a.ends_with("-f mp4 out.mp4"));
    }
}
