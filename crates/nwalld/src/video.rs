use anyhow::{anyhow, Context, Result};
use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const MAX_DECODE_EDGE: u32 = 1280;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Accel {
    Vaapi(&'static str),
    Cuda,
    Software,
}

const VAAPI_NODES: [&str; 2] = ["/dev/dri/renderD128", "/dev/dri/renderD129"];

fn accel_candidates() -> Vec<Accel> {
    let mut out: Vec<Accel> = VAAPI_NODES
        .iter()
        .filter(|n| Path::new(n).exists())
        .map(|n| Accel::Vaapi(n))
        .collect();
    out.push(Accel::Cuda);
    out
}

fn apply_accel(cmd: &mut Command, accel: Accel) {
    match accel {
        Accel::Vaapi(node) => {
            cmd.args([
                "-hwaccel",
                "vaapi",
                "-hwaccel_device",
                node,
                "-hwaccel_output_format",
                "vaapi",
            ]);
        }
        Accel::Cuda => {
            cmd.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
        }
        Accel::Software => {}
    }
}

fn accel_filter(accel: Accel, fps: u32, w: u32, h: u32) -> String {
    match accel {
        Accel::Vaapi(_) => {
            format!("fps={fps},scale_vaapi={w}:{h}:format=bgra,hwdownload,format=bgra")
        }
        Accel::Cuda => format!(
            "fps={fps},scale_cuda={w}:{h}:format=nv12,hwdownload,format=nv12,format=bgra"
        ),
        Accel::Software => format!(
            "fps={fps},scale={w}:{h}:flags=bilinear:force_original_aspect_ratio=disable,format=bgra"
        ),
    }
}

fn probe_accel(path: &Path, w: u32, h: u32) -> Accel {
    const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

    let forced = std::env::var("NWALL_ACCEL").unwrap_or_default();
    let candidates: Vec<Accel> = match forced.trim().to_ascii_lowercase().as_str() {
        "software" | "none" | "off" => return Accel::Software,
        "cuda" | "nvdec" => vec![Accel::Cuda],
        "vaapi" => VAAPI_NODES.iter().map(|n| Accel::Vaapi(n)).collect(),
        _ => accel_candidates(),
    };

    for accel in candidates {
        let mut cmd = ffmpeg_cmd();
        apply_accel(&mut cmd, accel);
        let spawned = cmd
            .args([
                "-i",
                &path.to_string_lossy(),
                "-an",
                "-vf",
                &accel_filter(accel, 1, w, h),
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "bgra",
                "-y",
                "/dev/null",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = spawned else { continue };
        if wait_child_timeout(&mut child, PROBE_TIMEOUT) {
            log::info!("hardware decode via {accel:?}");
            return accel;
        }
    }
    log::info!("no usable hardware decode; software");
    Accel::Software
}

pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
}

const FRAME_SLOTS: usize = 3;

fn acquire_slot(slots: &mut Vec<Arc<Vec<u8>>>, frame_bytes: usize) -> Arc<Vec<u8>> {
    for i in 0..slots.len() {
        if Arc::get_mut(&mut slots[i]).is_some() {
            return slots.swap_remove(i);
        }
    }
    Arc::new(vec![0u8; frame_bytes])
}

fn recycle_slot(slots: &mut Vec<Arc<Vec<u8>>>, buf: Arc<Vec<u8>>) {
    if slots.len() < FRAME_SLOTS {
        slots.push(buf);
    }
}

enum Ctrl {
    Stop,
    SetMute(bool),
    SetVolume(f32),
}

pub struct VideoPlayer {
    ctrl_tx: Sender<Ctrl>,
    frame_rx: Receiver<VideoFrame>,
    join: Option<JoinHandle<()>>,
    ffmpeg_pid: Arc<AtomicU32>,
    audio_pid: Arc<AtomicU32>,
    paused: Arc<AtomicBool>,
}

impl VideoPlayer {
    pub fn start(path: &Path, fps: u32, mute: bool, volume: f32) -> Result<Self> {
        let (ow, oh) = probe_size(path)?;
        let (dw, dh) = fit_inside(ow, oh, MAX_DECODE_EDGE);
        let fps = fps.clamp(1, 240);
        let (ctrl_tx, ctrl_rx) = mpsc::channel();
        let (frame_tx, frame_rx) = mpsc::sync_channel::<VideoFrame>(1);
        let path = path.to_path_buf();
        let ffmpeg_pid = Arc::new(AtomicU32::new(0));
        let audio_pid = Arc::new(AtomicU32::new(0));
        let paused = Arc::new(AtomicBool::new(false));

        log::info!("video {dw}x{dh} @ {fps}fps from {}x{}", ow, oh);

        let join = thread::Builder::new()
            .name("nwall-ffmpeg".into())
            .spawn({
                let ffmpeg_pid = Arc::clone(&ffmpeg_pid);
                let audio_pid = Arc::clone(&audio_pid);
                let paused = Arc::clone(&paused);
                move || {
                    if let Err(e) = decode_loop(
                        path, fps, mute, volume, dw, dh, ctrl_rx, frame_tx, ffmpeg_pid, audio_pid,
                        paused,
                    ) {
                        log::error!("video decode ended: {e:#}");
                    }
                }
            })?;

        Ok(Self {
            ctrl_tx,
            frame_rx,
            join: Some(join),
            ffmpeg_pid,
            audio_pid,
            paused,
        })
    }

    pub fn try_next_frame(&mut self) -> Option<VideoFrame> {
        match self.frame_rx.try_recv() {
            Ok(f) => Some(f),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    pub fn is_dead(&self) -> bool {
        self.join.as_ref().map(|j| j.is_finished()).unwrap_or(true)
    }

    pub fn pause(&mut self) {
        if self.paused.swap(true, Ordering::SeqCst) {
            return;
        }
        signal_group(self.ffmpeg_pid.load(Ordering::SeqCst), Signal::SIGSTOP);
        signal_group(self.audio_pid.load(Ordering::SeqCst), Signal::SIGSTOP);
    }

    pub fn resume(&mut self) {
        if !self.paused.swap(false, Ordering::SeqCst) {
            return;
        }
        signal_group(self.ffmpeg_pid.load(Ordering::SeqCst), Signal::SIGCONT);
        signal_group(self.audio_pid.load(Ordering::SeqCst), Signal::SIGCONT);
    }

    pub fn set_mute(&mut self, mute: bool) {
        let _ = self.ctrl_tx.send(Ctrl::SetMute(mute));
    }

    pub fn set_volume(&mut self, volume: f32) {
        let _ = self.ctrl_tx.send(Ctrl::SetVolume(volume));
    }

    pub fn audio_pid(&self) -> Option<u32> {
        let pid = self.audio_pid.load(Ordering::SeqCst);
        (pid != 0).then_some(pid)
    }

    pub fn stop(mut self) {
        self.shutdown(/* join */ true);
    }

    pub fn kill_detach(mut self) {
        self.shutdown(/* join */ false);
    }

    pub fn snapshot_png(path: &Path, out: &Path) -> Result<()> {
        const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(8);
        let mut cmd = ffmpeg_cmd();
        let mut child = cmd
            .args([
                "-y",
                "-ss",
                "0",
                "-i",
                path.to_str().unwrap_or(""),
                "-an",
                "-frames:v",
                "1",
                "-vf",
                "scale=1280:-2:flags=bilinear",
                out.to_str().unwrap_or(""),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("ffmpeg snapshot")?;
        if !wait_child_timeout(&mut child, SNAPSHOT_TIMEOUT) {
            let _ = std::fs::remove_file(out);
            return Err(anyhow!("ffmpeg snapshot failed"));
        }
        Ok(())
    }

    fn shutdown(&mut self, join: bool) {
        if self.join.is_none() {
            return;
        }
        self.paused.store(false, Ordering::SeqCst);
        signal_group(self.ffmpeg_pid.load(Ordering::SeqCst), Signal::SIGCONT);
        signal_group(self.audio_pid.load(Ordering::SeqCst), Signal::SIGCONT);
        let _ = self.ctrl_tx.send(Ctrl::Stop);
        signal_group(self.ffmpeg_pid.load(Ordering::SeqCst), Signal::SIGKILL);
        signal_group(self.audio_pid.load(Ordering::SeqCst), Signal::SIGKILL);
        self.ffmpeg_pid.store(0, Ordering::SeqCst);
        self.audio_pid.store(0, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            if join {
                let _ = j.join();
            } else {
                // Detach so the Wayland/IPC loop is not blocked.
                thread::spawn(move || {
                    let _ = j.join();
                });
            }
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.shutdown(/* join */ false);
    }
}

fn fit_inside(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    let m = w.max(h);
    if m <= max_edge {
        return (w & !1, h & !1);
    }
    let scale = max_edge as f64 / m as f64;
    (
        ((w as f64 * scale) as u32).max(2) & !1,
        ((h as f64 * scale) as u32).max(2) & !1,
    )
}

fn probe_size(path: &Path) -> Result<(u32, u32)> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
            path.to_str().ok_or_else(|| anyhow!("non-utf8 path"))?,
        ])
        .output()
        .context("run ffprobe")?;
    if !out.status.success() {
        return Err(anyhow!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim();
    let (w, h) = text
        .split_once('x')
        .ok_or_else(|| anyhow!("unexpected ffprobe output: {text}"))?;
    Ok((w.parse()?, h.parse()?))
}

fn signal_group(pid: u32, sig: Signal) {
    if pid == 0 {
        return;
    }
    let pgid = Pid::from_raw(pid as i32);
    let _ = killpg(pgid, sig);
    let _ = kill(pgid, sig);
}

fn remember_child(slot: &AtomicU32, child: &Child, paused: &AtomicBool) {
    let pid = child.id();
    slot.store(pid, Ordering::SeqCst);
    if paused.load(Ordering::SeqCst) {
        signal_group(pid, Signal::SIGSTOP);
    }
}

fn configure_child(cmd: &mut Command) {
    cmd.process_group(0);
    unsafe {
        cmd.pre_exec(|| {
            // SAFETY: runs in the forked child before exec.
            if nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn ffmpeg_cmd() -> Command {
    let mut cmd = Command::new("ffmpeg");
    configure_child(&mut cmd);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-threads",
        "1",
        "-filter_threads",
        "1",
    ]);
    cmd
}

fn wait_child_timeout(child: &mut Child, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if start.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(40));
            }
            _ => {
                let pid = child.id();
                signal_group(pid, Signal::SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

struct TrackedChild {
    child: Option<Child>,
    slot: Arc<AtomicU32>,
}

impl TrackedChild {
    fn new(child: Child, slot: Arc<AtomicU32>, paused: &AtomicBool) -> Self {
        remember_child(&slot, &child, paused);
        Self {
            child: Some(child),
            slot,
        }
    }

    fn kill_wait(&mut self) {
        if let Some(mut c) = self.child.take() {
            let pid = c.id();
            signal_group(pid, Signal::SIGCONT);
            signal_group(pid, Signal::SIGKILL);
            let _ = c.kill();
            let _ = c.wait();
        }
        self.slot.store(0, Ordering::SeqCst);
    }

    fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.as_mut().and_then(|c| c.stdout.take())
    }
}

impl Drop for TrackedChild {
    fn drop(&mut self) {
        self.kill_wait();
    }
}

fn spawn_ffmpeg(path: &Path, fps: u32, w: u32, h: u32, accel: Accel) -> Result<Child> {
    let vf = accel_filter(accel, fps, w, h);
    let mut cmd = ffmpeg_cmd();
    cmd.args(["-fflags", "+genpts"]);
    apply_accel(&mut cmd, accel);
    cmd.args([
        "-stream_loop",
        "-1",
        "-i",
        &path.to_string_lossy(),
        "-an",
        "-vf",
        &vf,
        "-f",
        "rawvideo",
        "-pix_fmt",
        "bgra",
        "pipe:1",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .context("spawn ffmpeg")
}

fn spawn_audio(path: &Path, volume: f32) -> Result<Child> {
    let vol = volume.clamp(0.0, 1.0);
    let mut cmd = ffmpeg_cmd();
    cmd.args([
        "-stream_loop",
        "-1",
        "-re",
        "-i",
        &path.to_string_lossy(),
        "-vn",
        "-af",
        &format!("volume={vol}"),
        "-f",
        "pulse",
        "nwall",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .context("spawn ffmpeg audio")
}

fn decode_loop(
    path: PathBuf,
    fps: u32,
    mut mute: bool,
    mut volume: f32,
    w: u32,
    h: u32,
    ctrl_rx: Receiver<Ctrl>,
    frame_tx: SyncSender<VideoFrame>,
    ffmpeg_pid: Arc<AtomicU32>,
    audio_pid: Arc<AtomicU32>,
    paused: Arc<AtomicBool>,
) -> Result<()> {
    let frame_bytes = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| anyhow!("frame too large"))?;

    let mut accel = probe_accel(&path, w, h);
    let child = match spawn_ffmpeg(&path, fps, w, h, accel) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("{accel:?} ffmpeg spawn failed ({e:#}); software");
            accel = Accel::Software;
            spawn_ffmpeg(&path, fps, w, h, accel)?
        }
    };
    let mut video = TrackedChild::new(child, Arc::clone(&ffmpeg_pid), &paused);
    let mut audio: Option<TrackedChild> = if mute {
        None
    } else {
        spawn_audio(&path, volume)
            .ok()
            .map(|c| TrackedChild::new(c, Arc::clone(&audio_pid), &paused))
    };
    if audio.is_none() {
        audio_pid.store(0, Ordering::SeqCst);
    }
    let mut stdout: ChildStdout = video
        .take_stdout()
        .ok_or_else(|| anyhow!("ffmpeg stdout missing"))?;
    let mut slots: Vec<Arc<Vec<u8>>> = Vec::with_capacity(FRAME_SLOTS);
    let mut consecutive_fail = 0u32;
    let interval = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let mut next_due = Instant::now();
    let mut need_restart = false;
    let mut pending_spawn = false;

    loop {
        while let Ok(msg) = ctrl_rx.try_recv() {
            match msg {
                Ctrl::Stop => return Ok(()),
                Ctrl::SetMute(m) => {
                    mute = m;
                    if mute {
                        if let Some(mut a) = audio.take() {
                            a.kill_wait();
                        }
                        audio_pid.store(0, Ordering::SeqCst);
                    } else if audio.is_none() {
                        audio = spawn_audio(&path, volume)
                            .ok()
                            .map(|c| TrackedChild::new(c, Arc::clone(&audio_pid), &paused));
                    }
                }
                Ctrl::SetVolume(v) => {
                    volume = v;
                    if !mute {
                        if let Some(mut a) = audio.take() {
                            a.kill_wait();
                        }
                        audio = spawn_audio(&path, volume)
                            .ok()
                            .map(|c| TrackedChild::new(c, Arc::clone(&audio_pid), &paused));
                    }
                }
            }
        }

        if need_restart {
            video.kill_wait();
            if consecutive_fail >= 2 && accel != Accel::Software {
                accel = Accel::Software;
                log::info!("hardware decode kept failing; switching to software");
            }
            thread::sleep(Duration::from_millis(80));
            need_restart = false;
            pending_spawn = true;
            continue;
        }

        if pending_spawn {
            if matches!(ctrl_rx.try_recv(), Ok(Ctrl::Stop)) {
                return Ok(());
            }
            video = TrackedChild::new(
                spawn_ffmpeg(&path, fps, w, h, accel)?,
                Arc::clone(&ffmpeg_pid),
                &paused,
            );
            stdout = video
                .take_stdout()
                .ok_or_else(|| anyhow!("stdout"))?;
            pending_spawn = false;
        }

        let mut slot = acquire_slot(&mut slots, frame_bytes);
        let read = {
            let dst = Arc::get_mut(&mut slot).expect("acquire_slot returns an unshared buffer");
            dst.resize(frame_bytes, 0);
            stdout.read_exact(dst.as_mut_slice())
        };
        if let Err(e) = read {
            if matches!(ctrl_rx.try_recv(), Ok(Ctrl::Stop)) {
                return Ok(());
            }
            consecutive_fail += 1;
            log::warn!("ffmpeg pipe ended ({e}); restart #{consecutive_fail} ({accel:?})");
            recycle_slot(&mut slots, slot);
            need_restart = true;
            continue;
        }
        consecutive_fail = 0;

        let mut frame = VideoFrame {
            width: w,
            height: h,
            bgra: Arc::clone(&slot),
        };
        recycle_slot(&mut slots, slot);
        loop {
            if matches!(ctrl_rx.try_recv(), Ok(Ctrl::Stop)) {
                return Ok(());
            }
            match frame_tx.try_send(frame) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(f)) => {
                    frame = f;
                    thread::sleep(Duration::from_millis(5));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return Ok(()),
            }
        }

        let now = Instant::now();
        if next_due > now {
            thread::sleep(next_due - now);
            next_due += interval;
        } else {
            next_due = now + interval;
        }
    }
}

pub fn cover_source(sw: u32, sh: u32, dw: u32, dh: u32) -> (f64, f64, f64, f64) {
    if dw == 0 || dh == 0 {
        return (0.0, 0.0, sw as f64, sh as f64);
    }
    let src_a = sw as f64 / sh as f64;
    let dst_a = dw as f64 / dh as f64;
    if src_a > dst_a {
        let crop_w = sh as f64 * dst_a;
        ((sw as f64 - crop_w) * 0.5, 0.0, crop_w, sh as f64)
    } else {
        let crop_h = sw as f64 / dst_a;
        (0.0, (sh as f64 - crop_h) * 0.5, sw as f64, crop_h)
    }
}
