//! CLI client for the nwall wallpaper daemon.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use nwall_ipc::{client_request, FitMode, PausePolicy, Request, Response, Slideshow};

#[derive(Parser, Debug)]
#[command(
    name = "nwall",
    about = "niri wallpaper daemon client (local files + settings)",
    long_about = "Control nwalld over the Unix socket. Use this (or raw IPC) to \
set local wallpapers, video playback, background music, smart pause, and \
slideshow — enough to build any GUI or script client. Online Discover sources \
are not part of this CLI."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the daemon in the background (if not already running)
    Daemon {
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(short, long)]
        wallpaper: Option<PathBuf>,
        #[arg(long)]
        foreground: bool,
    },
    /// Open the wallpaper picker GUI
    Gui,
    /// Check that nwalld is reachable
    Ping,
    /// Show daemon status as JSON (outputs, pause policy, music, …)
    Status,
    /// Set a local wallpaper (image or video)
    Set {
        path: PathBuf,
        /// Connector name (repeatable), e.g. -o HDMI-A-1 -o DP-3. Default: all.
        #[arg(short, long)]
        output: Vec<String>,
    },
    /// Pause video wallpaper playback (manual freeze)
    Pause,
    /// Resume video wallpaper playback
    Resume,
    /// Set video FPS
    Fps { fps: u32 },
    /// Mute / unmute video wallpaper audio
    Mute {
        #[arg(long)]
        off: bool,
    },
    /// Set video wallpaper volume 0.0–1.0 (or 0–100; also unmutes)
    Volume { volume: f32 },
    /// Set fit mode: cover | contain | stretch
    Fit { mode: String },
    /// Show or change smart-pause settings
    #[command(name = "pause-policy")]
    PausePolicy {
        #[arg(long)]
        fullscreen: Option<bool>,
        #[arg(long)]
        overview: Option<bool>,
        #[arg(long)]
        window_drag: Option<bool>,
        #[arg(long)]
        covered: Option<bool>,
        #[arg(long = "music-fullscreen")]
        music_fullscreen: Option<bool>,
        #[arg(long = "music-overview")]
        music_overview: Option<bool>,
        #[arg(long = "music-window-drag")]
        music_window_drag: Option<bool>,
        #[arg(long = "music-covered")]
        music_covered: Option<bool>,
        #[arg(long = "music-on-other-audio")]
        music_on_other_audio: Option<bool>,
    },
    /// Slideshow rotation (local library or a configured source name)
    Slideshow {
        #[command(subcommand)]
        action: SlideshowAction,
    },
    /// Per-wallpaper looping background music (local audio file)
    Music {
        #[command(subcommand)]
        action: MusicAction,
    },
    /// Reload ~/.config/nwall/config.toml into the running daemon
    Reload,
    /// Ask the daemon to quit
    Quit,
}

#[derive(Subcommand, Debug)]
enum SlideshowAction {
    /// Enable rotation and apply a wallpaper now
    Start,
    /// Disable rotation (keeps source, interval, and tags)
    Stop,
    /// Print current slideshow settings as JSON
    Show,
    /// Update slideshow settings (does not require Start)
    Set {
        /// Enable or disable rotation
        #[arg(long)]
        enable: Option<bool>,
        /// Minutes between rotations (1–1440)
        #[arg(long)]
        interval: Option<u32>,
        /// `library` or a catalog source name from config
        #[arg(long)]
        source: Option<String>,
        /// Tags / search string (filename match for library)
        #[arg(long)]
        tags: Option<String>,
        /// Show Start/Stop slideshow in the system tray
        #[arg(long)]
        tray: bool,
        /// Hide slideshow controls from the system tray
        #[arg(long)]
        no_tray: bool,
    },
}

#[derive(Subcommand, Debug)]
enum MusicAction {
    /// Attach a local audio file to a wallpaper
    Set {
        path: PathBuf,
        #[arg(long, short = 'w')]
        wallpaper: Option<PathBuf>,
    },
    /// Remove background music from a wallpaper
    Clear {
        #[arg(long, short = 'w')]
        wallpaper: Option<PathBuf>,
    },
    /// Set background-music volume 0.0–1.0 (or 0–100)
    Volume {
        volume: f32,
        #[arg(long, short = 'w')]
        wallpaper: Option<PathBuf>,
    },
    /// Mute background music (use --off to unmute)
    Mute {
        #[arg(long)]
        off: bool,
        #[arg(long, short = 'w')]
        wallpaper: Option<PathBuf>,
    },
    /// Pause background music (same as mute)
    Pause {
        #[arg(long, short = 'w')]
        wallpaper: Option<PathBuf>,
    },
    /// Resume background music (same as unmute)
    Resume {
        #[arg(long, short = 'w')]
        wallpaper: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Commands::Daemon {
            config,
            wallpaper,
            foreground,
        } => start_daemon(config, wallpaper, foreground),
        Commands::Gui => {
            let status = Command::new("nwall-gui").status();
            match status {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => Err(anyhow!("nwall-gui exited with {s}")),
                Err(e) => Err(anyhow!("failed to launch nwall-gui: {e}")),
            }
        }
        Commands::Ping => {
            ensure_daemon()?;
            client_request(&Request::Ping)?;
            println!("pong");
            Ok(())
        }
        Commands::Status => {
            ensure_daemon()?;
            let r = client_request(&Request::Status)?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            Ok(())
        }
        Commands::Set { path, output } => {
            ensure_daemon()?;
            print_resp(client_request(&Request::Set {
                path,
                output: None,
                outputs: output,
            })?)
        }
        Commands::Pause => {
            ensure_daemon()?;
            print_resp(client_request(&Request::Pause)?)
        }
        Commands::Resume => {
            ensure_daemon()?;
            print_resp(client_request(&Request::Resume)?)
        }
        Commands::Fps { fps } => {
            ensure_daemon()?;
            print_resp(client_request(&Request::SetFps { fps })?)
        }
        Commands::Mute { off } => {
            ensure_daemon()?;
            print_resp(client_request(&Request::SetMute { mute: !off })?)
        }
        Commands::Volume { volume } => {
            ensure_daemon()?;
            print_resp(client_request(&Request::SetVolume {
                volume: normalize_volume(volume),
            })?)
        }
        Commands::Fit { mode } => {
            ensure_daemon()?;
            let fit: FitMode = mode.parse()?;
            print_resp(client_request(&Request::SetFit { fit })?)
        }
        Commands::PausePolicy {
            fullscreen,
            overview,
            window_drag,
            covered,
            music_fullscreen,
            music_overview,
            music_window_drag,
            music_covered,
            music_on_other_audio,
        } => {
            ensure_daemon()?;
            let mut pause = match client_request(&Request::Status)? {
                Response::Status(s) => s.pause,
                _ => PausePolicy::default(),
            };
            let any = fullscreen.is_some()
                || overview.is_some()
                || window_drag.is_some()
                || covered.is_some()
                || music_fullscreen.is_some()
                || music_overview.is_some()
                || music_window_drag.is_some()
                || music_covered.is_some()
                || music_on_other_audio.is_some();
            if !any {
                println!("{}", serde_json::to_string_pretty(&pause)?);
                return Ok(());
            }
            if let Some(v) = fullscreen {
                pause.on_fullscreen = v;
            }
            if let Some(v) = overview {
                pause.on_overview = v;
            }
            if let Some(v) = window_drag {
                pause.on_window_drag = v;
            }
            if let Some(v) = covered {
                pause.on_covered = v;
            }
            if let Some(v) = music_fullscreen {
                pause.music_on_fullscreen = v;
            }
            if let Some(v) = music_overview {
                pause.music_on_overview = v;
            }
            if let Some(v) = music_window_drag {
                pause.music_on_window_drag = v;
            }
            if let Some(v) = music_covered {
                pause.music_on_covered = v;
            }
            if let Some(v) = music_on_other_audio {
                pause.music_on_other_audio = v;
            }
            print_resp(client_request(&Request::SetPausePolicy { pause })?)
        }
        Commands::Slideshow { action } => {
            ensure_daemon()?;
            match action {
                SlideshowAction::Start => {
                    print_resp(client_request(&Request::SlideshowStart)?)
                }
                SlideshowAction::Stop => {
                    print_resp(client_request(&Request::SlideshowStop)?)
                }
                SlideshowAction::Show => {
                    let ss = load_slideshow()?;
                    println!("{}", serde_json::to_string_pretty(&ss)?);
                    Ok(())
                }
                SlideshowAction::Set {
                    enable,
                    interval,
                    source,
                    tags,
                    tray,
                    no_tray,
                } => {
                    let mut ss = load_slideshow()?;
                    if let Some(v) = enable {
                        ss.enabled = v;
                    }
                    if let Some(v) = interval {
                        ss.interval_minutes = v;
                    }
                    if let Some(v) = source {
                        ss.source = v;
                    }
                    if let Some(v) = tags {
                        ss.tags = v;
                    }
                    if tray {
                        ss.show_in_tray = true;
                    } else if no_tray {
                        ss.show_in_tray = false;
                    }
                    ss.clamp_interval();
                    print_resp(client_request(&Request::SetSlideshow { slideshow: ss })?)
                }
            }
        }
        Commands::Music { action } => {
            ensure_daemon()?;
            match action {
                MusicAction::Set { path, wallpaper } => {
                    let wall = resolve_wallpaper(wallpaper)?;
                    print_resp(client_request(&Request::SetBgMusic {
                        wallpaper: wall,
                        music: Some(path),
                    })?)
                }
                MusicAction::Clear { wallpaper } => {
                    let wall = resolve_wallpaper(wallpaper)?;
                    print_resp(client_request(&Request::SetBgMusic {
                        wallpaper: wall,
                        music: None,
                    })?)
                }
                MusicAction::Volume { volume, wallpaper } => {
                    let wall = resolve_wallpaper(wallpaper)?;
                    print_resp(client_request(&Request::SetBgMusicVolume {
                        wallpaper: wall,
                        volume: normalize_volume(volume),
                    })?)
                }
                MusicAction::Mute { off, wallpaper } => print_resp(client_request(
                    &Request::SetBgMusicMute {
                        wallpaper,
                        mute: !off,
                    },
                )?),
                MusicAction::Pause { wallpaper } => print_resp(client_request(
                    &Request::SetBgMusicMute {
                        wallpaper,
                        mute: true,
                    },
                )?),
                MusicAction::Resume { wallpaper } => print_resp(client_request(
                    &Request::SetBgMusicMute {
                        wallpaper,
                        mute: false,
                    },
                )?),
            }
        }
        Commands::Reload => {
            ensure_daemon()?;
            print_resp(client_request(&Request::ReloadConfig)?)
        }
        Commands::Quit => print_resp(client_request(&Request::Quit)?),
    }
}

fn load_slideshow() -> Result<Slideshow> {
    match client_request(&Request::Status)? {
        Response::Status(s) => Ok(s.slideshow),
        _ => Ok(nwall_ipc::Config::load(&nwall_ipc::default_config_path())
            .unwrap_or_default()
            .slideshow),
    }
}

fn normalize_volume(v: f32) -> f32 {
    if v > 1.0 {
        (v / 100.0).clamp(0.0, 1.0)
    } else {
        v.clamp(0.0, 1.0)
    }
}

fn resolve_wallpaper(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    match client_request(&Request::Status)? {
        Response::Status(s) => s
            .outputs
            .iter()
            .find_map(|o| o.wallpaper.clone())
            .ok_or_else(|| anyhow!("no wallpaper applied — pass --wallpaper PATH")),
        other => Err(anyhow!("unexpected status response: {other:?}")),
    }
}

fn print_resp(r: Response) -> Result<()> {
    match r {
        Response::Ok { message, .. } => {
            if let Some(m) = message {
                println!("{m}");
            }
            Ok(())
        }
        Response::Error { error } => Err(anyhow!(error)),
        Response::Status(s) => {
            println!("{}", serde_json::to_string_pretty(&s)?);
            Ok(())
        }
    }
}

fn ensure_daemon() -> Result<()> {
    if client_request(&Request::Ping).is_ok() {
        return Ok(());
    }
    start_daemon(None, None, false)?;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if client_request(&Request::Ping).is_ok() {
            return Ok(());
        }
    }
    Err(anyhow!("nwalld did not become ready"))
}

fn start_daemon(
    config: Option<PathBuf>,
    wallpaper: Option<PathBuf>,
    foreground: bool,
) -> Result<()> {
    if client_request(&Request::Ping).is_ok() {
        eprintln!("nwalld already running");
        return Ok(());
    }
    let mut cmd = Command::new("nwalld");
    if let Some(c) = config {
        cmd.arg("--config").arg(c);
    }
    if let Some(w) = wallpaper {
        cmd.arg("--wallpaper").arg(w);
    }
    if foreground {
        let status = cmd.status().context("run nwalld")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("nwalld exited with {status}"))
        }
    } else {
        let log_path = nwall_ipc::daemon_log_path();
        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map(Stdio::from)
            .unwrap_or_else(|_| Stdio::null());
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .context("spawn nwalld")?;
        Ok(())
    }
}
