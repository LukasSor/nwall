
use anyhow::{Context, Result};
use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub struct BgMusicPlayer {
    path: PathBuf,
    volume: f32,
    mute: bool,
    /// Temporary pause from smart-pause (does not change sidecar mute).
    smart_paused: bool,
    child: Option<Child>,
}

impl BgMusicPlayer {
    pub fn start(path: &Path, volume: f32, mute: bool) -> Result<Self> {
        let path = std::fs::canonicalize(path)
            .with_context(|| format!("canonicalize music {}", path.display()))?;
        let volume = volume.clamp(0.0, 1.0);
        let child = if mute {
            None
        } else {
            Some(spawn_music(&path, volume)?)
        };
        Ok(Self {
            path,
            volume,
            mute,
            smart_paused: false,
            child,
        })
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn mute(&self) -> bool {
        self.mute
    }

    pub fn smart_paused(&self) -> bool {
        self.smart_paused
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }

    pub fn set_mute(&mut self, mute: bool) {
        if self.mute == mute {
            return;
        }
        self.mute = mute;
        self.sync_child();
    }

    pub fn set_smart_paused(&mut self, paused: bool) {
        if self.smart_paused == paused {
            return;
        }
        self.smart_paused = paused;
        self.sync_child();
    }

    pub fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        if (self.volume - volume).abs() < f32::EPSILON && self.child.is_some() == self.want_playing()
        {
            self.volume = volume;
            return;
        }
        self.volume = volume;
        if self.want_playing() {
            self.respawn();
        }
    }

    /// Same track / settings — no-op. Otherwise restart.
    pub fn ensure(&mut self, path: &Path, volume: f32, mute: bool) -> Result<()> {
        let path = std::fs::canonicalize(path)
            .with_context(|| format!("canonicalize music {}", path.display()))?;
        let volume = volume.clamp(0.0, 1.0);
        if self.path == path && (self.volume - volume).abs() < f32::EPSILON && self.mute == mute {
            self.sync_child();
            return Ok(());
        }
        self.kill_child();
        self.path = path;
        self.volume = volume;
        self.mute = mute;
        self.sync_child();
        Ok(())
    }

    pub fn stop(mut self) {
        self.kill_child();
    }

    fn want_playing(&self) -> bool {
        !self.mute && !self.smart_paused
    }

    fn sync_child(&mut self) {
        if self.want_playing() {
            if self.child.is_none() {
                self.respawn();
            }
        } else {
            self.kill_child();
        }
    }

    fn respawn(&mut self) {
        self.kill_child();
        match spawn_music(&self.path, self.volume) {
            Ok(c) => self.child = Some(c),
            Err(e) => log::warn!("bg music spawn {}: {e:#}", self.path.display()),
        }
    }

    fn kill_child(&mut self) {
        if let Some(mut c) = self.child.take() {
            let pid = c.id();
            signal_group(pid, Signal::SIGCONT);
            signal_group(pid, Signal::SIGKILL);
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for BgMusicPlayer {
    fn drop(&mut self) {
        self.kill_child();
    }
}

fn signal_group(pid: u32, sig: Signal) {
    if pid == 0 {
        return;
    }
    let pgid = Pid::from_raw(pid as i32);
    let _ = killpg(pgid, sig);
    let _ = kill(pgid, sig);
}

fn configure_child(cmd: &mut Command) {
    cmd.process_group(0);
    unsafe {
        cmd.pre_exec(|| {
            if nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn spawn_music(path: &Path, volume: f32) -> Result<Child> {
    let vol = volume.clamp(0.0, 1.0);
    let mut cmd = Command::new("ffmpeg");
    configure_child(&mut cmd);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
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
        "nwall-bg",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .context("spawn ffmpeg bg music")
}
