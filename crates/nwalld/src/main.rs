
mod bg_music;
mod niri;
mod other_audio;
mod render;
mod rotate;
#[cfg(feature = "tray")]
mod tray;
mod video;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction};
use calloop_wayland_source::WaylandSource;
use clap::Parser;
use image::RgbaImage;
use nwall_catalog as catalog;
use nwall_ipc::{
    client_request, is_audio, is_image, is_video, socket_path, Config, FitMode,
    OutputStatus, Request, Response, Status, BACKDROP_NAMESPACE, LIVE_NAMESPACE,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    delegate_simple,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState, SimpleGlobal},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer as ShmBuffer, SlotPool},
        Shm, ShmHandler,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface::WlSurface},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::WpViewporter,
};

use crate::bg_music::BgMusicPlayer;
use crate::niri::NiriWatcher;
use crate::video::VideoPlayer;

#[derive(Parser, Debug)]
#[command(name = "nwalld", about = "niri-optimized wallpaper daemon")]
struct Args {
    /// Path to config.toml
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Wallpaper to show immediately (overrides config)
    #[arg(short, long)]
    wallpaper: Option<PathBuf>,
    /// FPS cap for video (0 = uncapped pacing)
    #[arg(long)]
    fps: Option<u32>,
}

#[derive(Clone)]
struct WallpaperSource {
    path: PathBuf,
    #[allow(dead_code)]
    kind: SourceKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Image,
    Video,
}

const VIDEO_SLOTS: usize = 4;
const STILL_SLOTS: usize = 2;

struct SurfaceDraw {
    pool: Option<SlotPool>,
    ring: Vec<ShmBuffer>,
    dims: Option<(u32, u32)>,
}

impl SurfaceDraw {
    fn new() -> Self {
        Self {
            pool: None,
            ring: Vec::new(),
            dims: None,
        }
    }

    fn acquire(&mut self, shm: &Shm, w: u32, h: u32, max_slots: usize) -> Result<usize> {
        let stride = (w as usize).saturating_mul(4);
        let bytes = stride
            .checked_mul(h as usize)
            .ok_or_else(|| anyhow!("buffer too large"))?;

        if self.dims != Some((w, h)) {
            self.ring.clear();
            self.pool = None;
            self.dims = Some((w, h));
        }
        if self.pool.is_none() {
            self.pool = Some(
                SlotPool::new(bytes.saturating_mul(max_slots).max(4096), shm)
                    .context("create shm pool")?,
            );
        }

        let Self { pool, ring, .. } = self;
        let pool = pool.as_mut().expect("pool created above");
        for (i, buffer) in ring.iter().enumerate() {
            if buffer.canvas(&mut *pool).is_some() {
                return Ok(i);
            }
        }
        if ring.len() < max_slots {
            let (buffer, _) = pool
                .create_buffer(w as i32, h as i32, stride as i32, wl_shm::Format::Xrgb8888)
                .map_err(|e| anyhow!("shm buffer: {e}"))?;
            ring.push(buffer);
            return Ok(ring.len() - 1);
        }
        Err(anyhow!("all {max_slots} buffers still held by the compositor"))
    }

    fn canvas(&mut self, idx: usize) -> Result<&mut [u8]> {
        let Self { pool, ring, .. } = self;
        let pool = pool.as_mut().ok_or_else(|| anyhow!("no shm pool"))?;
        ring.get(idx)
            .ok_or_else(|| anyhow!("buffer index out of range"))?
            .canvas(pool)
            .ok_or_else(|| anyhow!("buffer became busy"))
    }

    // Only safe after the compositor committed another buffer.
    fn release(&mut self) {
        self.ring.clear();
        self.pool = None;
        self.dims = None;
    }
}

struct OutputSurfaces {
    name: String,
    _output: wl_output::WlOutput,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    live: LayerSurface,
    backdrop: LayerSurface,
    live_viewport: WpViewport,
    backdrop_viewport: WpViewport,
    live_configured: bool,
    backdrop_configured: bool,
    live_draw: SurfaceDraw,
    backdrop_draw: SurfaceDraw,
    wallpaper: Option<PathBuf>,
    frozen: bool,
    image_committed: bool,
    video_vp: Option<(u32, u32, u32, u32)>,
    video_commits: u32,
}

struct App {
    qh: QueueHandle<App>,
    config_path: PathBuf,
    config: Config,
    compositor: CompositorState,
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    viewporter: SimpleGlobal<WpViewporter, 1>,
    outputs: HashMap<u32, OutputSurfaces>,
    current: Option<WallpaperSource>,
    image_cache: HashMap<u32, (PathBuf, RgbaImage)>,
    video_frames: HashMap<PathBuf, (u32, u32, Arc<Vec<u8>>)>,
    videos: HashMap<PathBuf, VideoPlayer>,
    video_draw: HashMap<PathBuf, SurfaceDraw>,
    backdrop_rgba: Option<RgbaImage>,
    backdrop_dirty: bool,
    niri: Option<NiriWatcher>,
    globally_frozen: bool,
    freeze_reason: Option<String>,
    play_override: bool,
    play_override_reason: Option<String>,
    drag_until: HashMap<String, Instant>,
    needs_live_redraw: bool,
    backdrop_mapped: bool,
    exit: bool,
    present_ready: bool,
    present_wait_since: Option<Instant>,
    rotate_tx: calloop::channel::Sender<crate::rotate::RotateResult>,
    slideshow_due: Instant,
    slideshow_busy: Arc<AtomicBool>,
    #[cfg(feature = "tray")]
    tray: crate::tray::TrayHandle,
    #[cfg(feature = "tray")]
    tray_sync: Option<(bool, bool, bool, bool, bool, bool, bool)>,
    bg_music: Option<BgMusicPlayer>,
    other_audio: crate::other_audio::OtherAudioProbe,
}

fn load_video_still(path: &Path) -> Result<image::RgbaImage> {
    let mut dir = nwall_ipc::cache_dir();
    dir.push("stills");
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(format!("{}.jpg", nwall_ipc::hash_url(&path.to_string_lossy())));
    let stale = dest
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .zip(path.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|(snap, src)| snap < src)
        .unwrap_or(true);
    if stale || dest.metadata().map(|m| m.len() == 0).unwrap_or(true) {
        VideoPlayer::snapshot_png(path, &dest)?;
    }
    render::load_rgba(&dest)
}

fn rgba_from_bgra_frame(width: u32, height: u32, bgra: &[u8]) -> Result<RgbaImage> {
    let need = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| anyhow!("frame too large"))?;
    if bgra.len() < need {
        return Err(anyhow!("short BGRA frame"));
    }
    let mut rgba = vec![0u8; need];
    for (dst, src) in rgba.chunks_exact_mut(4).zip(bgra[..need].chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = 255;
    }
    RgbaImage::from_raw(width, height, rgba).ok_or_else(|| anyhow!("rgba from BGRA frame"))
}

fn kill_child_ffmpeg(except: Option<u32>) {
    let my = std::process::id().to_string();
    let out = std::process::Command::new("pgrep")
        .args(["-P", &my, "-x", "ffmpeg"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(out) = out else {
        return;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(pid) = line.trim().parse::<u32>() else {
            continue;
        };
        if except == Some(pid) {
            continue;
        }
        let pgid = nix::unistd::Pid::from_raw(pid as i32);
        let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
        let _ = nix::sys::signal::kill(pgid, nix::sys::signal::Signal::SIGKILL);
    }
}

fn kill_gui() {
    let _ = std::process::Command::new("pkill")
        .args(["-x", "nwall-gui"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn init_logging() {
    let path = nwall_ipc::daemon_log_path();
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
        Err(e) => {
            eprintln!("nwalld: could not open log {}: {e}", path.display());
        }
    }
    builder.init();
    log::info!("logging to {}", path.display());
}

fn main() -> Result<()> {
    init_logging();
    std::thread::spawn(catalog::prune_disk_caches);
    let _ = std::process::Command::new("pkill")
        .args(["-x", "mpvpaper"])
        .status();
    let args = Args::parse();

    let config_path = args
        .config
        .unwrap_or_else(nwall_ipc::default_config_path);
    let mut config = Config::load(&config_path).unwrap_or_default();
    if let Some(fps) = args.fps {
        config.fps = fps;
    }
    if let Some(wp) = args.wallpaper {
        config.wallpaper = Some(wp);
    }

    if !config_path.exists() {
        config.save(&config_path)?;
        log::info!("wrote default config to {}", config_path.display());
    }

    let conn = Connection::connect_to_env().context("connect to Wayland")?;
    let (globals, event_queue) = registry_queue_init(&conn)?;
    let qh: QueueHandle<App> = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor")?;
    let output_state = OutputState::new(&globals, &qh);
    let registry_state = RegistryState::new(&globals);
    let shm = Shm::bind(&globals, &qh).context("wl_shm")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("layer-shell")?;
    let viewporter = SimpleGlobal::<WpViewporter, 1>::bind(&globals, &qh)
        .context("wp_viewporter")?;

    let niri = match NiriWatcher::start() {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn!("niri watcher unavailable ({e:#}); smart pause limited");
            None
        }
    };

    let (rotate_tx, rotate_rx) = calloop::channel::channel();
    #[cfg(feature = "tray")]
    let (tray_tx, tray_rx) = calloop::channel::channel();
    #[cfg(feature = "tray")]
    let tray = crate::tray::spawn(tray_tx);
    let ss_mins = config.slideshow.interval_minutes.max(1).min(1440) as u64;
    let mut app = App {
        qh: qh.clone(),
        config_path: config_path.clone(),
        config,
        compositor,
        registry_state,
        output_state,
        shm,
        layer_shell,
        viewporter,
        outputs: HashMap::new(),
        current: None,
        image_cache: HashMap::new(),
        video_frames: HashMap::new(),
        videos: HashMap::new(),
        video_draw: HashMap::new(),
        backdrop_rgba: None,
        backdrop_dirty: false,
        niri,
        globally_frozen: false,
        freeze_reason: None,
        play_override: false,
        play_override_reason: None,
        drag_until: HashMap::new(),
        needs_live_redraw: false,
        backdrop_mapped: false,
        exit: false,
        present_ready: true,
        present_wait_since: None,
        rotate_tx,
        slideshow_due: Instant::now() + Duration::from_secs(ss_mins.saturating_mul(60)),
        slideshow_busy: Arc::new(AtomicBool::new(false)),
        #[cfg(feature = "tray")]
        tray,
        #[cfg(feature = "tray")]
        tray_sync: None,
        bg_music: None,
        other_audio: crate::other_audio::OtherAudioProbe::default(),
    };

    let mut event_loop: EventLoop<'_, App> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| anyhow!("wayland source: {e}"))?;

    let listener = create_listener()?;
    loop_handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, app| {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Err(e) = handle_ipc_client(stream, app) {
                            log::warn!("ipc client error: {e:#}");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => log::warn!("ipc accept: {e}"),
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow!("insert ipc: {e}"))?;

    #[cfg(feature = "tray")]
    loop_handle
        .insert_source(tray_rx, |event, _, app| {
            if let calloop::channel::Event::Msg(cmd) = event {
                match cmd {
                    crate::tray::TrayCmd::Pause => app.pause_video(),
                    crate::tray::TrayCmd::Resume => app.resume_video(),
                    crate::tray::TrayCmd::Quit => {
                        app.request_quit();
                    }
                    crate::tray::TrayCmd::OpenGui => crate::tray::open_gui(),
                    crate::tray::TrayCmd::StartSlideshow => {
                        let _ = app.handle_request(Request::SlideshowStart);
                    }
                    crate::tray::TrayCmd::StopSlideshow => {
                        let _ = app.handle_request(Request::SlideshowStop);
                    }
                    crate::tray::TrayCmd::PauseBgMusic => {
                        let _ = app.handle_request(Request::SetBgMusicMute {
                            wallpaper: None,
                            mute: true,
                        });
                    }
                    crate::tray::TrayCmd::ResumeBgMusic => {
                        let _ = app.handle_request(Request::SetBgMusicMute {
                            wallpaper: None,
                            mute: false,
                        });
                    }
                }
            }
        })
        .map_err(|e| anyhow!("insert tray: {e}"))?;

    loop_handle
        .insert_source(rotate_rx, |event, _, app| {
            if let calloop::channel::Event::Msg(msg) = event {
                match msg {
                    crate::rotate::RotateResult::Path(path) => {
                        match app.set_wallpaper(path, Vec::new()) {
                            Ok(()) => {
                                app.arm_slideshow_due();
                                if app.globally_frozen {
                                    log::info!("slideshow: wallpaper applied while paused");
                                } else {
                                    log::info!("slideshow: wallpaper rotated");
                                }
                            }
                            Err(e) => {
                                log::warn!("slideshow apply: {e:#}");
                                app.arm_slideshow_retry();
                            }
                        }
                    }
                    crate::rotate::RotateResult::Failed(e) => {
                        log::warn!("slideshow: {e}; retry in 30s");
                        app.arm_slideshow_retry();
                    }
                }
                app.slideshow_busy.store(false, Ordering::SeqCst);
            }
        })
        .map_err(|e| anyhow!("insert rotate: {e}"))?;

    loop_handle
        .insert_source(
            Timer::from_duration(Duration::from_secs(5)),
            |_, _, app| {
                app.maybe_rotate();
                if app.exit {
                    TimeoutAction::Drop
                } else {
                    TimeoutAction::ToDuration(Duration::from_secs(5))
                }
            },
        )
        .map_err(|e| anyhow!("insert slideshow timer: {e}"))?;

    loop_handle
        .insert_source(Timer::from_duration(Duration::from_millis(8)), |_, _, app| {
            app.on_tick();
            if app.exit {
                TimeoutAction::Drop
            } else {
                TimeoutAction::ToDuration(app.tick_interval())
            }
        })
        .map_err(|e| anyhow!("insert timer: {e}"))?;

    let initial = app.config.wallpaper.clone();
    loop_handle
        .insert_source(Timer::from_duration(Duration::from_millis(100)), move |_, _, app| {
            let outs: Vec<_> = app
                .output_state
                .outputs()
                .map(|o| (o.id().protocol_id(), o))
                .collect();
            let qh = app.qh.clone();
            for (id, output) in outs {
                app.ensure_output(id, &output, &qh);
            }
            if let Some(path) = initial.clone().or_else(|| app.config.wallpaper.clone()) {
                if let Err(e) = app.set_wallpaper(path, Vec::new()) {
                    log::error!("initial wallpaper: {e:#}");
                }
            }
            TimeoutAction::Drop
        })
        .map_err(|e| anyhow!("insert init timer: {e}"))?;

    log::info!(
        "nwalld running (live=`{LIVE_NAMESPACE}`, backdrop=`{BACKDROP_NAMESPACE}`, socket={})",
        socket_path().display()
    );

    let signal = event_loop.get_signal();
    event_loop.run(None, &mut app, move |app| {
        if app.exit {
            signal.stop();
        }
    })?;
    for (_, v) in app.videos.drain() {
        v.kill_detach();
    }
    if let Some(m) = app.bg_music.take() {
        m.stop();
    }
    kill_child_ffmpeg(None);
    nwall_ipc_cleanup();
    Ok(())
}

fn create_listener() -> Result<UnixListener> {
    let path = socket_path();
    if path.exists() {
        if client_request(&Request::Ping).is_ok() {
            return Err(anyhow!(
                "nwalld already running at {}",
                path.display()
            ));
        }
        let _ = std::fs::remove_file(&path);
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind {}", path.display()))?;
    listener.set_nonblocking(true)?;
    log::info!("IPC on {}", path.display());
    Ok(listener)
}

fn nwall_ipc_cleanup() {
    let _ = std::fs::remove_file(socket_path());
}

fn handle_ipc_client(stream: UnixStream, app: &mut App) -> Result<()> {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let req: Request = serde_json::from_str(line.trim()).unwrap_or(Request::Ping);
    let resp = app.handle_request(req);
    let mut stream = stream;
    let out = serde_json::to_string(&resp)? + "\n";
    stream.write_all(out.as_bytes())?;
    Ok(())
}

impl App {
    fn handle_request(&mut self, req: Request) -> Response {
        match req {
            Request::Ping => Response::ok(),
            Request::Status => Response::Status(self.status()),
            Request::Set {
                path,
                output,
                outputs,
            } => {
                let mut names = outputs;
                if let Some(o) = output {
                    names.push(o);
                }
                match self.set_wallpaper(path, names) {
                    Ok(()) => Response::ok_msg("set"),
                    Err(e) => Response::err(format!("{e:#}")),
                }
            }
            Request::SetFps { fps } => {
                if self.config.fps != fps {
                    self.config.fps = fps;
                    let _ = self.config.save_preserving_gui(&self.config_path);
                    let running: Vec<PathBuf> = self.videos.keys().cloned().collect();
                    for p in running {
                        if let Some(v) = self.videos.remove(&p) {
                            v.stop();
                        }
                        self.video_frames.remove(&p);
                        self.video_draw.remove(&p);
                    }
                    if fps > 0 {
                        if let Some(cur) = self.current.as_mut() {
                            if is_video(&cur.path) {
                                cur.kind = SourceKind::Video;
                            }
                        }
                    }
                    if let Err(e) = self.resync_videos() {
                        return Response::err(format!("{e:#}"));
                    }
                    if fps == 0 {
                        let except = self.bg_music.as_ref().and_then(|m| m.pid());
                        kill_child_ffmpeg(except);
                        if let Err(e) = self.present_current_as_still() {
                            return Response::err(format!("{e:#}"));
                        }
                    }
                    self.sync_tray();
                }
                Response::ok()
            }
            Request::SetMute { mute } => {
                self.config.mute = mute;
                let _ = self.config.save_preserving_gui(&self.config_path);
                for v in self.videos.values_mut() {
                    v.set_mute(mute);
                }
                Response::ok()
            }
            Request::SetVolume { volume } => {
                self.config.volume = volume.clamp(0.0, 1.0);
                if self.config.volume > 0.0 {
                    self.config.mute = false;
                }
                let _ = self.config.save_preserving_gui(&self.config_path);
                for v in self.videos.values_mut() {
                    v.set_volume(self.config.volume);
                    v.set_mute(self.config.mute);
                }
                Response::ok()
            }
            Request::SetFit { fit } => {
                self.config.fit = fit;
                let _ = self.config.save_preserving_gui(&self.config_path);
                for o in self.outputs.values_mut() {
                    o.image_committed = false;
                }
                self.needs_live_redraw = true;
                Response::ok()
            }
            Request::SetPausePolicy { pause } => {
                self.config.pause = pause;
                let _ = self.config.save_preserving_gui(&self.config_path);
                self.sync_bg_music_smart_pause();
                Response::ok()
            }
            Request::SetSlideshow { mut slideshow } => {
                slideshow.clamp_interval();
                slideshow.source = nwall_ipc::canonicalize_source_name(&slideshow.source);
                let prev = self.config.slideshow.clone();
                let turning_on = !prev.enabled && slideshow.enabled;
                let interval_changed = prev.interval_minutes != slideshow.interval_minutes;
                self.config.slideshow = slideshow;
                let _ = self.config.save_preserving_gui(&self.config_path);
                if turning_on || (interval_changed && self.config.slideshow.enabled) {
                    self.arm_slideshow_due();
                }
                if prev.enabled && !self.config.slideshow.enabled {
                    log::info!("slideshow: disabled");
                } else if self.config.slideshow.enabled
                    && (turning_on
                        || interval_changed
                        || prev.source != self.config.slideshow.source
                        || prev.enabled != self.config.slideshow.enabled)
                {
                    log::info!(
                        "slideshow: every {} min from {} tags={:?}",
                        self.config.slideshow.interval_minutes,
                        self.config.slideshow.source,
                        self.config.slideshow.tags
                    );
                }
                self.sync_tray();
                Response::ok()
            }
            Request::SlideshowStart => {
                self.config.slideshow.enabled = true;
                let _ = self.config.save_preserving_gui(&self.config_path);
                log::info!(
                    "slideshow: start now, then every {} min from {} tags={:?}",
                    self.config.slideshow.interval_minutes,
                    self.config.slideshow.source,
                    self.config.slideshow.tags
                );
                self.spawn_slideshow_pick();
                self.sync_tray();
                Response::ok_msg("slideshow started")
            }
            Request::SlideshowStop => {
                self.config.slideshow.enabled = false;
                let _ = self.config.save_preserving_gui(&self.config_path);
                log::info!("slideshow: stopped");
                self.sync_tray();
                Response::ok_msg("slideshow stopped")
            }
            Request::Pause => {
                self.pause_video();
                Response::ok()
            }
            Request::Resume => {
                self.resume_video();
                Response::ok()
            }
            Request::ReloadConfig => match Config::load(&self.config_path) {
                Ok(c) => {
                    self.config = c;
                    self.arm_slideshow_due();
                    Response::ok()
                }
                Err(e) => Response::err(format!("{e:#}")),
            },
            Request::Quit => {
                self.request_quit();
                Response::ok_msg("bye")
            }
            Request::SetBgMusic { wallpaper, music } => {
                match self.set_bg_music(wallpaper, music) {
                    Ok(()) => Response::ok_msg("bg music updated"),
                    Err(e) => Response::err(format!("{e:#}")),
                }
            }
            Request::SetBgMusicVolume { wallpaper, volume } => {
                match self.set_bg_music_volume(wallpaper, volume) {
                    Ok(()) => Response::ok(),
                    Err(e) => Response::err(format!("{e:#}")),
                }
            }
            Request::SetBgMusicMute { wallpaper, mute } => {
                match self.set_bg_music_mute(wallpaper, mute) {
                    Ok(()) => Response::ok(),
                    Err(e) => Response::err(format!("{e:#}")),
                }
            }
        }
    }

    fn status(&self) -> Status {
        let outputs = self
            .outputs
            .values()
            .map(|o| OutputStatus {
                name: o.name.clone(),
                width: o.width,
                height: o.height,
                x: o.x,
                y: o.y,
                wallpaper: o
                    .wallpaper
                    .clone()
                    .or_else(|| self.current.as_ref().map(|c| c.path.clone())),
                frozen: !self.play_override && (o.frozen || self.globally_frozen),
                kind: o
                    .wallpaper
                    .as_ref()
                    .or_else(|| self.current.as_ref().map(|c| &c.path))
                    .map(|p| {
                        if is_video(p) {
                            "video"
                        } else if is_image(p) {
                            "image"
                        } else {
                            "none"
                        }
                    })
                    .unwrap_or("none")
                    .into(),
            })
            .collect();
        let (bg_active, bg_mute, bg_vol) = self.bg_music_status();
        Status {
            running: true,
            fps: self.config.fps,
            mute: self.config.mute,
            volume: self.config.volume,
            fit: self.config.fit,
            pause: self.config.pause.clone(),
            globally_frozen: self.globally_frozen,
            freeze_reason: self.freeze_reason.clone(),
            outputs,
            bg_music_active: bg_active,
            bg_music_mute: bg_mute,
            bg_music_volume: bg_vol,
            slideshow: self.config.slideshow.clone(),
        }
    }

    fn set_wallpaper(&mut self, path: PathBuf, outputs: Vec<String>) -> Result<()> {
        let path = std::fs::canonicalize(&path)
            .with_context(|| format!("canonicalize {}", path.display()))?;
        if !is_image(&path) && !is_video(&path) {
            return Err(anyhow!(
                "unsupported file type: {}",
                path.extension().and_then(|e| e.to_str()).unwrap_or("?")
            ));
        }

        let targets: Vec<String> = if outputs.is_empty() {
            self.outputs.values().map(|o| o.name.clone()).collect()
        } else {
            outputs
        };
        if targets.is_empty() {
            return Err(anyhow!("no outputs to apply to"));
        }
        let matched: Vec<u32> = self
            .outputs
            .iter()
            .filter(|(_, o)| targets.iter().any(|n| n == &o.name))
            .map(|(id, _)| *id)
            .collect();
        if matched.is_empty() {
            let have: Vec<&str> = self.outputs.values().map(|o| o.name.as_str()).collect();
            return Err(anyhow!(
                "no matching outputs for [{}] (have: {})",
                targets.join(", "),
                have.join(", ")
            ));
        }

        let play_video = is_video(&path) && self.config.fps > 0;

        let mut next_wanted: HashSet<PathBuf> = HashSet::new();
        if self.config.fps > 0 && !self.globally_frozen {
            for (id, o) in &self.outputs {
                let p = if matched.contains(id) {
                    Some(path.as_path())
                } else {
                    o.wallpaper.as_deref()
                };
                if let Some(p) = p {
                    if is_video(p) {
                        next_wanted.insert(p.to_path_buf());
                    }
                }
            }
        }
        let stale: Vec<PathBuf> = self
            .videos
            .keys()
            .filter(|p| !next_wanted.contains(*p))
            .cloned()
            .collect();
        for p in stale {
            if let Some(v) = self.videos.remove(&p) {
                v.stop();
            }
            self.video_frames.remove(&p);
            self.video_draw.remove(&p);
        }

        let still = if is_video(&path) {
            match load_video_still(&path) {
                Ok(rgba) => {
                    if !play_video {
                        log::info!("fps 0: still frame from {}", path.display());
                    }
                    Some(rgba)
                }
                Err(e) if play_video => {
                    log::warn!("video still {}: {e:#}", path.display());
                    None
                }
                Err(e) => return Err(e),
            }
        } else {
            Some(render::load_rgba(&path)?)
        };

        for name in &targets {
            self.config.outputs.insert(name.clone(), path.clone());
        }
        for id in &matched {
            if let Some(o) = self.outputs.get_mut(id) {
                o.wallpaper = Some(path.clone());
                o.image_committed = false;
                o.video_vp = None;
            }
            self.image_cache.remove(id);
        }
        self.config.wallpaper = Some(path.clone());
        let _ = self.config.save_preserving_gui(&self.config_path);
        self.current = Some(WallpaperSource {
            path: path.clone(),
            kind: if play_video {
                SourceKind::Video
            } else {
                SourceKind::Image
            },
        });

        let wanted_videos: HashSet<PathBuf> = self
            .outputs
            .values()
            .filter_map(|o| {
                o.wallpaper
                    .as_ref()
                    .filter(|p| is_video(p))
                    .cloned()
            })
            .collect();
        self.video_frames.retain(|p, _| wanted_videos.contains(p));

        if let Some(rgba) = still {
            self.backdrop_rgba = Some(render::make_backdrop(&rgba, 640, 2)?);
            for id in &matched {
                self.image_cache.insert(*id, (path.clone(), rgba.clone()));
            }
            self.needs_live_redraw = true;
            self.draw_lives();
            self.needs_live_redraw = true;
            if play_video {
                log::info!("video on {}", targets.join(", "));
            } else {
                log::info!("image on {}", targets.join(", "));
            }
        }

        self.ensure_backdrops_drawn();
        self.resync_videos()?;
        self.sync_bg_music();
        self.sync_tray();
        Ok(())
    }

    fn present_current_as_still(&mut self) -> Result<()> {
        let Some(path) = self.current.as_ref().map(|c| c.path.clone()) else {
            return Ok(());
        };
        if !is_video(&path) {
            return Ok(());
        }
        if let Some(cur) = self.current.as_mut() {
            cur.kind = SourceKind::Image;
        }
        log::info!("fps 0: still frame from {}", path.display());
        let rgba = load_video_still(&path)?;
        self.backdrop_rgba = Some(render::make_backdrop(&rgba, 640, 2)?);
        let ids: Vec<u32> = self
            .outputs
            .iter()
            .filter(|(_, o)| o.wallpaper.as_ref() == Some(&path))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.image_cache.insert(id, (path.clone(), rgba.clone()));
            if let Some(o) = self.outputs.get_mut(&id) {
                o.image_committed = false;
                o.video_vp = None;
            }
        }
        self.needs_live_redraw = true;
        self.draw_lives();
        self.needs_live_redraw = true;
        self.ensure_backdrops_drawn();
        Ok(())
    }

    fn resync_videos(&mut self) -> Result<()> {
        let mut wanted: HashSet<PathBuf> = HashSet::new();
        if self.config.fps > 0 && !self.globally_frozen {
            for o in self.outputs.values() {
                if let Some(p) = o.wallpaper.as_ref() {
                    if is_video(p) {
                        wanted.insert(p.clone());
                    }
                }
            }
        }
        let stale: Vec<PathBuf> = self
            .videos
            .keys()
            .filter(|p| !wanted.contains(*p))
            .cloned()
            .collect();
        for p in stale {
            if let Some(v) = self.videos.remove(&p) {
                v.stop();
            }
            self.video_frames.remove(&p);
            self.video_draw.remove(&p);
        }
        for p in wanted {
            if self.videos.get(&p).is_some_and(|v| !v.is_dead()) {
                continue;
            }
            if let Some(old) = self.videos.remove(&p) {
                old.stop();
            }
            let player = VideoPlayer::start(
                &p,
                self.config.fps,
                self.config.mute,
                self.config.volume,
            )?;
            log::info!("decoder {} @ {}fps", p.display(), self.config.fps);
            self.videos.insert(p, player);
        }
        self.sync_decoder_pauses();
        Ok(())
    }

    fn stop_all_decoders(&mut self) {
        for (_, v) in self.videos.drain() {
            v.kill_detach();
        }
        self.video_frames.clear();
        self.video_draw.clear();
        let except = self.bg_music.as_ref().and_then(|m| m.pid());
        kill_child_ffmpeg(except);
    }

    fn freeze_videos_as_stills(&mut self) -> Result<()> {
        let paths: HashSet<PathBuf> = self
            .outputs
            .values()
            .filter_map(|o| {
                o.wallpaper
                    .as_ref()
                    .filter(|p| is_video(p))
                    .cloned()
            })
            .collect();
        if paths.is_empty() {
            self.stop_all_decoders();
            return Ok(());
        }

        let mut stills: HashMap<PathBuf, RgbaImage> = HashMap::new();
        for path in &paths {
            let rgba = if let Some((w, h, bgra)) = self.video_frames.get(path) {
                match rgba_from_bgra_frame(*w, *h, bgra) {
                    Ok(img) => img,
                    Err(e) => {
                        log::warn!("pause frame {}: {e:#}; using file still", path.display());
                        load_video_still(path)?
                    }
                }
            } else {
                load_video_still(path)?
            };
            stills.insert(path.clone(), rgba);
        }

        self.stop_all_decoders();

        if let Some(rgba) = stills.values().next() {
            self.backdrop_rgba = Some(render::make_backdrop(rgba, 640, 2)?);
        }
        let ids: Vec<(u32, PathBuf)> = self
            .outputs
            .iter()
            .filter_map(|(id, o)| {
                let path = o.wallpaper.as_ref()?;
                stills.contains_key(path).then(|| (*id, path.clone()))
            })
            .collect();
        for (id, path) in ids {
            if let Some(rgba) = stills.get(&path) {
                self.image_cache.insert(id, (path, rgba.clone()));
            }
            if let Some(o) = self.outputs.get_mut(&id) {
                o.image_committed = false;
                o.video_vp = None;
            }
        }
        self.needs_live_redraw = true;
        self.draw_lives();
        self.needs_live_redraw = true;
        self.ensure_backdrops_drawn();
        Ok(())
    }

    fn ensure_output(&mut self, id: u32, output: &wl_output::WlOutput, qh: &QueueHandle<App>) {
        if self.outputs.contains_key(&id) {
            return;
        }
        let info = self.output_state.info(output);
        let name = info
            .as_ref()
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| format!("output-{id}"));
        let (width, height) = info
            .as_ref()
            .and_then(|i| i.logical_size)
            .map(|(w, h)| (w as u32, h as u32))
            .unwrap_or((0, 0));
        let (x, y) = info
            .as_ref()
            .and_then(|i| i.logical_position)
            .unwrap_or((0, 0));

        let live = self.make_layer(output, LIVE_NAMESPACE, qh);
        let backdrop = self.make_layer(output, BACKDROP_NAMESPACE, qh);
        let vp = self.viewporter.get().expect("viewporter");
        let live_viewport = vp.get_viewport(live.wl_surface(), qh, ());
        let backdrop_viewport = vp.get_viewport(backdrop.wl_surface(), qh, ());

        let wallpaper = self
            .config
            .outputs
            .get(&name)
            .cloned()
            .or_else(|| self.config.wallpaper.clone());

        log::info!("output {name} {width}x{height}");
        self.outputs.insert(
            id,
            OutputSurfaces {
                name,
                _output: output.clone(),
                width,
                height,
                x,
                y,
                live,
                backdrop,
                live_viewport,
                backdrop_viewport,
                live_configured: false,
                backdrop_configured: false,
                live_draw: SurfaceDraw::new(),
                backdrop_draw: SurfaceDraw::new(),
                wallpaper,
                frozen: false,
                image_committed: false,
                video_vp: None,
                video_commits: 0,
            },
        );
        self.needs_live_redraw = true;
        self.backdrop_dirty = true;
    }

    fn make_layer(
        &self,
        output: &wl_output::WlOutput,
        namespace: &str,
        qh: &QueueHandle<App>,
    ) -> LayerSurface {
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some(namespace),
            Some(output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        layer.commit();
        layer
    }

    fn on_tick(&mut self) {
        self.evaluate_freeze();
        self.sync_bg_music_smart_pause();

        if !self.present_ready {
            if self
                .present_wait_since
                .map(|t| t.elapsed() > Duration::from_millis(100))
                .unwrap_or(false)
            {
                self.present_ready = true;
                self.present_wait_since = None;
            }
        }

        // Wait for niri frame callback before commit.
        if self.present_ready && (self.play_override || !self.globally_frozen) {
            let paths: Vec<PathBuf> = self.videos.keys().cloned().collect();
            for path in paths {
                if !self.play_override && !self.video_needs_frames(&path) {
                    continue;
                }
                if let Some(player) = self.videos.get_mut(&path) {
                    if let Some(f) = player.try_next_frame() {
                        self.video_frames.insert(path, (f.width, f.height, f.bgra));
                        self.needs_live_redraw = true;
                    }
                }
            }
        }

        if self.needs_live_redraw && self.present_ready {
            self.draw_lives();
            let pending_still = self.outputs.values().any(|o| {
                let Some(p) = o.wallpaper.as_ref() else {
                    return false;
                };
                if o.image_committed {
                    return false;
                }
                !is_video(p) || !self.video_frames.contains_key(p)
            });
            if !pending_still {
                self.needs_live_redraw = false;
            }
        }
        if self.backdrop_dirty {
            self.ensure_backdrops_drawn();
        }
    }

    fn refresh_niri_freeze(&mut self) -> Option<String> {
        let policy = self.config.pause.clone();
        let mut reason: Option<String> = None;

        if let Some(w) = &self.niri {
            let _ = w.take_dirty();
            let snap = w.snapshot();
            let now = Instant::now();

            if policy.on_window_drag || policy.music_on_window_drag {
                for name in &snap.dragging_outputs {
                    self.drag_until
                        .insert(name.clone(), now + Duration::from_millis(200));
                }
            }
            self.drag_until.retain(|_, t| now < *t);

            let overview = policy.on_overview && snap.overview_open;
            for o in self.outputs.values_mut() {
                let fs = policy.on_fullscreen
                    && snap.fullscreen_outputs.iter().any(|n| n == &o.name);
                let cov =
                    policy.on_covered && snap.covered_outputs.iter().any(|n| n == &o.name);
                let drag = policy.on_window_drag
                    && (snap.dragging_outputs.iter().any(|n| n == &o.name)
                        || self.drag_until.contains_key(&o.name));
                o.frozen = overview || fs || cov || drag;
            }

            if overview {
                reason = Some("overview".into());
            } else if policy.on_window_drag
                && (!snap.dragging_outputs.is_empty() || !self.drag_until.is_empty())
            {
                reason = Some("window-drag".into());
            } else if policy.on_fullscreen && !snap.fullscreen_outputs.is_empty() {
                reason = Some("fullscreen".into());
            } else if policy.on_covered && !snap.covered_outputs.is_empty() {
                reason = Some("covered".into());
            }
        }
        reason
    }

    fn evaluate_freeze(&mut self) {
        let prev_per_out: Vec<(String, bool)> = self
            .outputs
            .values()
            .map(|o| (o.name.clone(), o.frozen))
            .collect();
        let reason = self.refresh_niri_freeze();

        let override_holds = self.play_override && reason == self.play_override_reason;
        if self.freeze_reason.as_deref() != Some("manual") && !override_holds {
            if self.play_override {
                log::info!(
                    "playback: continue override ended ({} → {})",
                    self.play_override_reason.as_deref().unwrap_or("none"),
                    reason.as_deref().unwrap_or("none")
                );
                self.play_override = false;
                self.play_override_reason = None;
            }
            let reason_changed = self.freeze_reason != reason;
            if reason_changed {
                self.freeze_reason = reason.clone();
            }
            let per_out_changed = self.outputs.values().any(|o| {
                prev_per_out
                    .iter()
                    .find(|(n, _)| n == &o.name)
                    .map(|(_, was)| *was != o.frozen)
                    .unwrap_or(o.frozen)
            });
            if per_out_changed {
                let frozen: Vec<&str> = self
                    .outputs
                    .values()
                    .filter(|o| o.frozen)
                    .map(|o| o.name.as_str())
                    .collect();
                if frozen.is_empty() {
                    log::info!("smart-pause: per-output off");
                } else {
                    log::info!(
                        "smart-pause: per-output frozen [{}] ({})",
                        frozen.join(", "),
                        reason.as_deref().unwrap_or("?")
                    );
                }
            } else if reason_changed && reason.is_none() {
                log::info!("smart-pause: per-output off");
            }
            if reason_changed || per_out_changed {
                self.sync_decoder_pauses();
                self.sync_bg_music_smart_pause();
                self.sync_tray();
            }
        }
    }

    fn slideshow_interval(&self) -> Duration {
        let mins = self.config.slideshow.interval_minutes.max(1).min(1440) as u64;
        Duration::from_secs(mins.saturating_mul(60))
    }

    fn arm_slideshow_due(&mut self) {
        self.slideshow_due = Instant::now() + self.slideshow_interval();
    }

    fn arm_slideshow_retry(&mut self) {
        self.slideshow_due = Instant::now() + Duration::from_secs(30);
    }

    fn wallpaper_is_video(&self) -> bool {
        if let Some(c) = &self.current {
            return c.kind == SourceKind::Video;
        }
        self.outputs.values().any(|o| {
            o.wallpaper
                .as_ref()
                .map(|p| is_video(p))
                .unwrap_or(false)
        })
    }

    #[cfg_attr(not(feature = "tray"), allow(dead_code))]
    fn any_video_animating(&self) -> bool {
        if self.config.fps == 0 || !self.wallpaper_is_video() {
            return false;
        }
        if self.play_override {
            return true;
        }
        if self.globally_frozen {
            return false;
        }
        self.outputs.values().any(|o| {
            o.wallpaper
                .as_ref()
                .map(|p| is_video(p))
                .unwrap_or(false)
                && !o.frozen
        })
    }

    fn sync_tray(&mut self) {
        #[cfg(feature = "tray")]
        {
            let video = self.wallpaper_is_video();
            let paused = video && !self.any_video_animating();
            let (bg_music, bg_mute, _) = self.bg_music_status();
            let snap = (
                self.config.slideshow.enabled,
                self.config.slideshow.show_in_tray,
                crate::tray::gui_installed(),
                video,
                paused,
                bg_music,
                bg_mute,
            );
            if self.tray_sync == Some(snap) {
                return;
            }
            self.tray_sync = Some(snap);
            self.tray.set(snap.0, snap.1, snap.2, snap.3, snap.4, snap.5, snap.6);
        }
    }

    fn bg_music_wallpaper(&self) -> Option<PathBuf> {
        self.config
            .wallpaper
            .clone()
            .or_else(|| self.current.as_ref().map(|c| c.path.clone()))
    }

    fn bg_music_status(&self) -> (bool, bool, f32) {
        if let Some(m) = &self.bg_music {
            return (true, m.mute(), m.volume());
        }
        let Some(wall) = self.bg_music_wallpaper() else {
            return (false, false, 0.5);
        };
        match catalog::load_path_meta(&wall) {
            Some(s) if s.bg_music.as_ref().is_some_and(|p| p.is_file()) => {
                (true, s.bg_music_muted(), s.bg_music_volume_or_default())
            }
            _ => (false, false, 0.5),
        }
    }

    fn sync_bg_music(&mut self) {
        let Some(wall) = self.bg_music_wallpaper() else {
            if let Some(m) = self.bg_music.take() {
                m.stop();
            }
            return;
        };
        let meta = catalog::load_path_meta(&wall);
        let Some(music) = meta
            .as_ref()
            .and_then(|s| s.bg_music.clone())
            .filter(|p| p.is_file())
        else {
            if let Some(m) = self.bg_music.take() {
                m.stop();
            }
            return;
        };
        let volume = meta
            .as_ref()
            .map(|s| s.bg_music_volume_or_default())
            .unwrap_or(0.5);
        let mute = meta.as_ref().map(|s| s.bg_music_muted()).unwrap_or(false);
        if let Some(player) = self.bg_music.as_mut() {
            if let Err(e) = player.ensure(&music, volume, mute) {
                log::warn!("bg music: {e:#}");
                if let Some(m) = self.bg_music.take() {
                    m.stop();
                }
            }
        } else {
            match BgMusicPlayer::start(&music, volume, mute) {
                Ok(p) => {
                    log::info!("bg music {}", music.display());
                    self.bg_music = Some(p);
                }
                Err(e) => log::warn!("bg music start {}: {e:#}", music.display()),
            }
        }
        self.sync_bg_music_smart_pause();
    }

    fn sync_bg_music_smart_pause(&mut self) {
        let want = self.bg_music_should_smart_pause();
        if let Some(m) = self.bg_music.as_mut() {
            let was = m.smart_paused();
            m.set_smart_paused(want);
            if was != want {
                log::info!(
                    "bg music smart-pause {}",
                    if want { "on" } else { "off" }
                );
            }
        }
    }

    fn bg_music_should_smart_pause(&mut self) -> bool {
        let policy = self.config.pause.clone();
        if policy.any_music_niri_trigger() && self.globally_frozen && !self.play_override {
            return true;
        }
        if let Some(w) = &self.niri {
            let snap = w.snapshot();
            if policy.music_on_overview && snap.overview_open {
                return true;
            }
            if policy.music_on_fullscreen && !snap.fullscreen_outputs.is_empty() {
                return true;
            }
            if policy.music_on_covered && !snap.covered_outputs.is_empty() {
                return true;
            }
            if policy.music_on_window_drag
                && (!snap.dragging_outputs.is_empty() || !self.drag_until.is_empty())
            {
                return true;
            }
        }
        if policy.music_on_other_audio {
            let mut except = Vec::new();
            if let Some(pid) = self.bg_music.as_ref().and_then(|m| m.pid()) {
                except.push(pid);
            }
            for v in self.videos.values() {
                if let Some(pid) = v.audio_pid() {
                    except.push(pid);
                }
            }
            if self.other_audio.active(&except) {
                return true;
            }
        }
        false
    }

    fn set_bg_music(&mut self, wallpaper: PathBuf, music: Option<PathBuf>) -> Result<()> {
        let wallpaper = std::fs::canonicalize(&wallpaper)
            .with_context(|| format!("canonicalize {}", wallpaper.display()))?;
        if !is_image(&wallpaper) && !is_video(&wallpaper) {
            return Err(anyhow!("not a wallpaper file: {}", wallpaper.display()));
        }
        let music = match music {
            Some(p) => {
                let p = std::fs::canonicalize(&p)
                    .with_context(|| format!("canonicalize music {}", p.display()))?;
                if !is_audio(&p) {
                    return Err(anyhow!(
                        "unsupported audio type: {}",
                        p.extension().and_then(|e| e.to_str()).unwrap_or("?")
                    ));
                }
                Some(p)
            }
            None => None,
        };
        catalog::set_bg_music_meta(&wallpaper, music.as_deref(), None, None);
        if self.bg_music_wallpaper().as_ref() == Some(&wallpaper) {
            self.sync_bg_music();
            self.sync_tray();
        }
        Ok(())
    }

    fn set_bg_music_volume(&mut self, wallpaper: PathBuf, volume: f32) -> Result<()> {
        let wallpaper = std::fs::canonicalize(&wallpaper)
            .with_context(|| format!("canonicalize {}", wallpaper.display()))?;
        let volume = volume.clamp(0.0, 1.0);
        catalog::set_bg_music_volume_meta(&wallpaper, volume);
        if self.bg_music_wallpaper().as_ref() == Some(&wallpaper) {
            if let Some(m) = self.bg_music.as_mut() {
                m.set_volume(volume);
                if volume > 0.0 {
                    m.set_mute(false);
                    catalog::set_bg_music_mute_meta(&wallpaper, false);
                }
            } else {
                self.sync_bg_music();
            }
            self.sync_tray();
        }
        Ok(())
    }

    fn set_bg_music_mute(&mut self, wallpaper: Option<PathBuf>, mute: bool) -> Result<()> {
        let wallpaper = match wallpaper {
            Some(p) => std::fs::canonicalize(&p)
                .with_context(|| format!("canonicalize {}", p.display()))?,
            None => self
                .bg_music_wallpaper()
                .ok_or_else(|| anyhow!("no wallpaper with background music"))?,
        };
        let meta = catalog::load_path_meta(&wallpaper);
        if meta.as_ref().and_then(|s| s.bg_music.as_ref()).is_none() {
            return Err(anyhow!("no background music on {}", wallpaper.display()));
        }
        catalog::set_bg_music_mute_meta(&wallpaper, mute);
        if self.bg_music_wallpaper().as_ref() == Some(&wallpaper) {
            if let Some(m) = self.bg_music.as_mut() {
                m.set_mute(mute);
            } else {
                self.sync_bg_music();
            }
            self.sync_tray();
        }
        Ok(())
    }

    fn maybe_rotate(&mut self) {
        if !self.config.slideshow.enabled {
            return;
        }
        if self.slideshow_busy.load(Ordering::SeqCst) {
            return;
        }
        if Instant::now() < self.slideshow_due {
            return;
        }
        log::info!(
            "slideshow: interval due, picking from {}",
            self.config.slideshow.source
        );
        self.spawn_slideshow_pick();
    }

    fn spawn_slideshow_pick(&mut self) {
        nwall_ipc::ensure_builtin_sources(&mut self.config.sources);
        let current = self.current.as_ref().map(|c| c.path.clone());
        crate::rotate::spawn_pick(
            self.config.clone(),
            current,
            Arc::clone(&self.slideshow_busy),
            self.rotate_tx.clone(),
        );
    }

    fn request_quit(&mut self) {
        if self.exit {
            return;
        }
        self.exit = true;
        #[cfg(feature = "tray")]
        self.tray.set_quitting();
        kill_gui();
        for (_, v) in self.videos.drain() {
            v.kill_detach();
        }
        if let Some(m) = self.bg_music.take() {
            m.stop();
        }
        kill_child_ffmpeg(None);
        log::info!("quit requested");
    }

    fn pause_video(&mut self) {
        if self.config.fps == 0 || !self.wallpaper_is_video() {
            return;
        }
        if self.globally_frozen && self.freeze_reason.as_deref() == Some("manual") {
            self.sync_tray();
            return;
        }
        self.play_override = false;
        self.play_override_reason = None;
        self.globally_frozen = true;
        self.freeze_reason = Some("manual".into());
        if let Err(e) = self.freeze_videos_as_stills() {
            log::warn!("pause still: {e:#}");
            self.stop_all_decoders();
        }
        self.sync_tray();
        self.sync_bg_music_smart_pause();
        log::info!("playback: pause (manual, still — decoder stopped)");
    }

    fn resume_video(&mut self) {
        if self.config.fps == 0 || !self.wallpaper_is_video() {
            return;
        }
        let reason = self.refresh_niri_freeze();
        self.play_override = true;
        self.play_override_reason = reason.clone();
        self.globally_frozen = false;
        self.freeze_reason = reason;
        if let Err(e) = self.resync_videos() {
            log::warn!("resume decoder: {e:#}");
        }
        self.needs_live_redraw = true;
        self.sync_tray();
        self.sync_bg_music_smart_pause();
        log::info!(
            "playback: continue (override {})",
            self.play_override_reason.as_deref().unwrap_or("none")
        );
    }

    fn set_decoders_paused(&mut self, paused: bool) {
        for v in self.videos.values_mut() {
            if paused {
                v.pause();
            } else {
                v.resume();
            }
        }
    }

    fn tick_interval(&self) -> Duration {
        const ACTIVE: Duration = Duration::from_millis(8);
        const FROZEN: Duration = Duration::from_millis(32);
        const STATIC: Duration = Duration::from_millis(100);

        if self.needs_live_redraw || !self.present_ready || self.backdrop_dirty {
            return ACTIVE;
        }
        if self.videos.is_empty() {
            return STATIC;
        }
        if self.play_override {
            return ACTIVE;
        }
        if self.globally_frozen || !self.videos.keys().any(|p| self.video_needs_frames(p)) {
            return FROZEN;
        }
        ACTIVE
    }

    fn video_needs_frames(&self, path: &Path) -> bool {
        self.outputs.values().any(|o| {
            o.wallpaper.as_ref().map(|p| p.as_path()) == Some(path) && !o.frozen
        })
    }

    fn sync_decoder_pauses(&mut self) {
        if self.play_override {
            self.set_decoders_paused(false);
            return;
        }
        if self.globally_frozen {
            self.set_decoders_paused(true);
            return;
        }
        let paths: Vec<PathBuf> = self.videos.keys().cloned().collect();
        for path in paths {
            let needed = self.video_needs_frames(&path);
            if let Some(v) = self.videos.get_mut(&path) {
                if needed {
                    v.resume();
                } else {
                    v.pause();
                }
            }
        }
    }

    fn apply_surface_hints(&self, layer: &LayerSurface, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        if let Ok(r) = Region::new(&self.compositor) {
            r.add(0, 0, w, h);
            layer.set_opaque_region(Some(r.wl_region()));
        }
        // Empty input region: clicks pass through to niri.
        if let Ok(empty) = Region::new(&self.compositor) {
            layer.set_input_region(Some(empty.wl_region()));
        }
    }

    fn ensure_backdrops_drawn(&mut self) {
        if self.backdrop_rgba.is_none() {
            if let Err(e) = self.refresh_backdrop_from_live() {
                log::debug!("backdrop still: {e:#}");
                return;
            }
        }
        let ready = self
            .outputs
            .values()
            .any(|o| o.backdrop_configured && o.width > 0 && o.height > 0);
        if !ready {
            self.backdrop_dirty = true;
            return;
        }
        self.draw_backdrops();
        self.backdrop_mapped = true;
        self.backdrop_dirty = false;
    }

    fn refresh_backdrop_from_live(&mut self) -> Result<()> {
        if let Some((_, rgba)) = self.image_cache.values().next() {
            self.backdrop_rgba = Some(render::make_backdrop(rgba, 640, 2)?);
            return Ok(());
        }
        for (path, (w, h, bgra)) in &self.video_frames {
            let rgba = rgba_from_bgra_frame(*w, *h, bgra)?;
            self.backdrop_rgba = Some(render::make_backdrop(&rgba, 640, 2)?);
            let _ = path;
            return Ok(());
        }
        Err(anyhow!("no live frame to build overview backdrop"))
    }

    fn draw_lives(&mut self) {
        let video_paths: Vec<PathBuf> = self.video_frames.keys().cloned().collect();
        for path in video_paths {
            if let Err(e) = self.present_video_shared(&path) {
                log::warn!("draw video {}: {e:#}", path.display());
            }
        }

        let fit = self.config.fit;
        let keys: Vec<u32> = self.outputs.keys().copied().collect();
        let mut requested_frame = false;
        for id in keys {
            let (w, h, surface, wp) = {
                let Some(o) = self.outputs.get(&id) else {
                    continue;
                };
                if !o.live_configured || o.width == 0 || o.height == 0 {
                    continue;
                }
                (
                    o.width,
                    o.height,
                    o.live.wl_surface().clone(),
                    o.wallpaper.clone(),
                )
            };
            let Some(wp) = wp else { continue };
            if is_video(&wp) && self.video_frames.contains_key(&wp) {
                continue;
            }
            if self
                .outputs
                .get(&id)
                .map(|o| o.image_committed)
                .unwrap_or(true)
            {
                continue;
            }
            let Some(rgba) = self.image_cache.get(&id).and_then(|(cached, rgba)| {
                (cached == &wp).then_some(rgba.clone())
            }) else {
                continue;
            };
            let was_ready = self.present_ready;
            let with_frame = !requested_frame;
            if let Err(e) =
                self.draw_image_surface(id, true, &surface, &rgba, w, h, fit, with_frame)
            {
                log::warn!("draw live: {e:#}");
            } else {
                if with_frame {
                    requested_frame = true;
                }
                if was_ready {
                    if let Some(o) = self.outputs.get_mut(&id) {
                        o.image_committed = true;
                    }
                } else {
                    self.needs_live_redraw = true;
                }
            }
        }
    }

    fn present_video_shared(&mut self, path: &Path) -> Result<()> {
        let Some((vw, vh, bgra)) = self.video_frames.get(path).cloned() else {
            return Ok(());
        };
        let mut targets: Vec<(u32, u32, u32, WlSurface)> = Vec::new();
        for (id, o) in &self.outputs {
            let wp = o.wallpaper.as_ref();
            if wp.map(|p| p.as_path()) != Some(path) {
                continue;
            }
            if !o.live_configured || o.width == 0 || o.height == 0 {
                continue;
            }
            if !self.play_override
                && (o.frozen || self.globally_frozen)
                && o.video_vp.is_some()
            {
                continue;
            }
            targets.push((
                *id,
                o.width,
                o.height,
                o.live.wl_surface().clone(),
            ));
        }
        if targets.is_empty() {
            return Ok(());
        }

        let draw = self
            .video_draw
            .entry(path.to_path_buf())
            .or_insert_with(SurfaceDraw::new);
        let idx = match draw.acquire(&self.shm, vw, vh, VIDEO_SLOTS) {
            Ok(i) => i,
            Err(e) => {
                log::debug!("video present skipped: {e}");
                return Ok(());
            }
        };
        {
            let canvas = draw.canvas(idx)?;
            if canvas.len() < bgra.len() {
                return Err(anyhow!("canvas too small"));
            }
            canvas[..bgra.len()].copy_from_slice(&bgra);
        }
        let wl_buf = draw.ring[idx].wl_buffer().clone();

        let qh = self.qh.clone();
        let mut requested_frame = false;
        for (id, ow, oh, surface) in targets {
            let vp_key = (vw, vh, ow, oh);
            if let Some(out) = self.outputs.get_mut(&id) {
                if out.video_vp != Some(vp_key) {
                    let (sx, sy, sw, sh) = video::cover_source(vw, vh, ow, oh);
                    out.live_viewport
                        .set_source(sx.into(), sy.into(), sw.into(), sh.into());
                    out.live_viewport.set_destination(ow as i32, oh as i32);
                    out.video_vp = Some(vp_key);
                }
            }
            if !requested_frame {
                surface.frame(&qh, surface.clone());
                requested_frame = true;
            }
            surface.attach(Some(&wl_buf), 0, 0);
            surface.damage_buffer(0, 0, vw as i32, vh as i32);
            surface.commit();
            if let Some(out) = self.outputs.get_mut(&id) {
                out.video_commits = out.video_commits.saturating_add(1);
                if out.video_commits == 2 {
                    out.live_draw.release();
                }
            }
        }
        if requested_frame {
            self.present_ready = false;
            self.present_wait_since = Some(Instant::now());
        }
        Ok(())
    }

    fn draw_backdrops(&mut self) {
        let Some(rgba) = self.backdrop_rgba.clone() else {
            return;
        };
        let keys: Vec<u32> = self.outputs.keys().copied().collect();
        for id in keys {
            let (w, h, surface) = {
                let Some(o) = self.outputs.get(&id) else {
                    continue;
                };
                if !o.backdrop_configured || o.width == 0 || o.height == 0 {
                    continue;
                }
                (o.width, o.height, o.backdrop.wl_surface().clone())
            };
            if let Err(e) =
                self.draw_image_surface(id, false, &surface, &rgba, w, h, FitMode::Cover, false)
            {
                log::warn!("draw backdrop: {e:#}");
            }
        }
    }

    fn draw_image_surface(
        &mut self,
        output_id: u32,
        live: bool,
        surface: &WlSurface,
        rgba: &RgbaImage,
        width: u32,
        height: u32,
        fit: FitMode,
        with_frame: bool,
    ) -> Result<()> {
        if width == 0 || height == 0 || rgba.width() == 0 || rgba.height() == 0 {
            return Ok(());
        }

        let (buf_w, buf_h, pixels, source) = match fit {
            FitMode::Contain => {
                let scaled = render::scale_fit(rgba, width, height, fit)?;
                (width, height, scaled, None)
            }
            FitMode::Cover => {
                let (sx, sy, sw, sh) =
                    video::cover_source(rgba.width(), rgba.height(), width, height);
                (
                    rgba.width(),
                    rgba.height(),
                    rgba.as_raw().clone(),
                    Some((sx, sy, sw, sh)),
                )
            }
            FitMode::Stretch => (
                rgba.width(),
                rgba.height(),
                rgba.as_raw().clone(),
                Some((0.0, 0.0, rgba.width() as f64, rgba.height() as f64)),
            ),
        };

        let qh = self.qh.clone();
        {
            let Some(out) = self.outputs.get_mut(&output_id) else {
                return Ok(());
            };
            let (draw, viewport) = if live {
                (&mut out.live_draw, &out.live_viewport)
            } else {
                (&mut out.backdrop_draw, &out.backdrop_viewport)
            };

            let idx = match draw.acquire(&self.shm, buf_w, buf_h, STILL_SLOTS) {
                Ok(i) => i,
                Err(e) => {
                    log::debug!("still present deferred: {e}");
                    return Ok(());
                }
            };
            {
                let canvas = draw.canvas(idx)?;
                if canvas.len() < pixels.len() {
                    return Err(anyhow!("canvas too small"));
                }
                for (dst, src) in canvas.chunks_exact_mut(4).zip(pixels.chunks_exact(4)) {
                    dst[0] = src[2];
                    dst[1] = src[1];
                    dst[2] = src[0];
                    dst[3] = 255;
                }
            }
            let wl_buf = draw.ring[idx].wl_buffer().clone();

            match source {
                Some((sx, sy, sw, sh)) => {
                    viewport.set_source(sx.into(), sy.into(), sw.into(), sh.into());
                }
                None => {
                    viewport.set_source((-1.0).into(), (-1.0).into(), (-1.0).into(), (-1.0).into());
                }
            }
            viewport.set_destination(width as i32, height as i32);

            if with_frame {
                surface.frame(&qh, surface.clone());
            }
            surface.attach(Some(&wl_buf), 0, 0);
            surface.damage_buffer(0, 0, buf_w as i32, buf_h as i32);
            surface.damage(0, 0, i32::MAX, i32::MAX);
            surface.commit();
        }
        if live {
            if let Some(out) = self.outputs.get_mut(&output_id) {
                out.video_commits = 0;
            }
        }
        if with_frame {
            self.present_ready = false;
            self.present_wait_since = Some(Instant::now());
        }
        Ok(())
    }
}


impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: wayland_client::protocol::wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _time: u32,
    ) {
        self.present_ready = true;
        self.present_wait_since = None;
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let id = output.id().protocol_id();
        self.ensure_output(id, &output, qh);
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let id = output.id().protocol_id();
        if let Some(info) = self.output_state.info(&output) {
            if let Some(o) = self.outputs.get_mut(&id) {
                if let Some((w, h)) = info.logical_size {
                    o.width = w as u32;
                    o.height = h as u32;
                    o.image_committed = false;
                    self.needs_live_redraw = true;
                }
                if let Some((x, y)) = info.logical_position {
                    o.x = x;
                    o.y = y;
                }
            }
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let id = output.id().protocol_id();
        self.outputs.remove(&id);
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {}

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let w = configure.new_size.0;
        let h = configure.new_size.1;
        let mut hints: Vec<(LayerSurface, u32, u32)> = Vec::new();
        for o in self.outputs.values_mut() {
            if o.live.wl_surface() == layer.wl_surface() {
                if w > 0 && h > 0 {
                    o.width = w;
                    o.height = h;
                }
                o.live_configured = true;
                o.image_committed = false;
                o.video_vp = None;
                self.needs_live_redraw = true;
                self.present_ready = true;
                hints.push((o.live.clone(), o.width, o.height));
            }
            if o.backdrop.wl_surface() == layer.wl_surface() {
                if w > 0 && h > 0 {
                    o.width = w;
                    o.height = h;
                }
                o.backdrop_configured = true;
                self.backdrop_dirty = true;
                self.needs_live_redraw = true;
                hints.push((o.backdrop.clone(), o.width, o.height));
            }
        }
        for (surf, ow, oh) in hints {
            self.apply_surface_hints(&surf, ow as i32, oh as i32);
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl AsMut<SimpleGlobal<WpViewporter, 1>> for App {
    fn as_mut(&mut self) -> &mut SimpleGlobal<WpViewporter, 1> {
        &mut self.viewporter
    }
}

impl Dispatch<WpViewport, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_layer!(App);
delegate_registry!(App);
delegate_simple!(App, WpViewporter, 1);
