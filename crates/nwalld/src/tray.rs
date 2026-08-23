use std::process::Command;

use calloop::channel::Sender;

#[derive(Debug, Clone, Copy)]
pub enum TrayCmd {
    Pause,
    Resume,
    StartSlideshow,
    StopSlideshow,
    PauseBgMusic,
    ResumeBgMusic,
    OpenGui,
    Quit,
}

struct NwallTray {
    tx: Sender<TrayCmd>,
    slideshow: bool,
    show_slideshow: bool,
    show_gui: bool,
    video: bool,
    paused: bool,
    bg_music: bool,
    bg_music_mute: bool,
    quitting: bool,
}

pub struct TrayHandle(ksni::Handle<NwallTray>);

impl TrayHandle {
    pub fn set(
        &self,
        slideshow: bool,
        show_slideshow: bool,
        show_gui: bool,
        video: bool,
        paused: bool,
        bg_music: bool,
        bg_music_mute: bool,
    ) {
        self.0.update(|t| {
            if t.quitting {
                return;
            }
            t.slideshow = slideshow;
            t.show_slideshow = show_slideshow;
            t.show_gui = show_gui;
            t.video = video;
            t.paused = paused;
            t.bg_music = bg_music;
            t.bg_music_mute = bg_music_mute;
        });
    }

    pub fn set_quitting(&self) {
        self.0.update(|t| {
            t.quitting = true;
        });
    }
}

impl ksni::Tray for NwallTray {
    fn id(&self) -> String {
        "nwall".into()
    }

    fn title(&self) -> String {
        if self.quitting {
            "nwall — quitting".into()
        } else {
            "nwall".into()
        }
    }

    fn icon_name(&self) -> String {
        "nwall".into()
    }

    fn icon_theme_path(&self) -> String {
        icon_dir().to_string_lossy().into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![make_tray_icon(22), make_tray_icon(32)]
    }

    fn status(&self) -> ksni::Status {
        ksni::Status::Active
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: if self.quitting {
                "nwall — quitting".into()
            } else {
                "nwall".into()
            },
            description: if self.quitting {
                "Shutting down…".into()
            } else {
                "niri wallpaper".into()
            },
            icon_name: "nwall".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        if self.quitting || !self.show_gui {
            return;
        }
        let _ = self.tx.send(TrayCmd::OpenGui);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        if self.quitting {
            return vec![StandardItem {
                label: "Quitting…".into(),
                enabled: false,
                icon_data: menu_icon_png(MenuGlyph::Quit),
                ..Default::default()
            }
            .into()];
        }
        let item = |label: String, glyph: MenuGlyph, tx: Sender<TrayCmd>, cmd: TrayCmd| {
            StandardItem {
                label,
                icon_data: menu_icon_png(glyph),
                activate: Box::new(move |t: &mut NwallTray| {
                    if t.quitting {
                        return;
                    }
                    if matches!(cmd, TrayCmd::Quit) {
                        t.quitting = true;
                    }
                    let _ = tx.send(cmd);
                }),
                ..Default::default()
            }
            .into()
        };
        let mut items = Vec::new();
        if self.show_gui {
            items.push(item(
                "Open picker".into(),
                MenuGlyph::Open,
                self.tx.clone(),
                TrayCmd::OpenGui,
            ));
        }
        if self.video {
            if !items.is_empty() {
                items.push(MenuItem::Separator);
            }
            if self.paused {
                items.push(item(
                    "Continue video".into(),
                    MenuGlyph::Play,
                    self.tx.clone(),
                    TrayCmd::Resume,
                ));
            } else {
                items.push(item(
                    "Pause video".into(),
                    MenuGlyph::Pause,
                    self.tx.clone(),
                    TrayCmd::Pause,
                ));
            }
        }
        if self.bg_music {
            if !self.video && (self.show_gui || !items.is_empty()) {
                items.push(MenuItem::Separator);
            }
            if self.bg_music_mute {
                items.push(item(
                    "Resume background music".into(),
                    MenuGlyph::Play,
                    self.tx.clone(),
                    TrayCmd::ResumeBgMusic,
                ));
            } else {
                items.push(item(
                    "Pause background music".into(),
                    MenuGlyph::Pause,
                    self.tx.clone(),
                    TrayCmd::PauseBgMusic,
                ));
            }
        }
        if self.show_slideshow {
            items.push(MenuItem::Separator);
            if self.slideshow {
                items.push(item(
                    "Stop slideshow".into(),
                    MenuGlyph::Stop,
                    self.tx.clone(),
                    TrayCmd::StopSlideshow,
                ));
            } else {
                items.push(item(
                    "Start slideshow".into(),
                    MenuGlyph::Play,
                    self.tx.clone(),
                    TrayCmd::StartSlideshow,
                ));
            }
        }
        items.push(MenuItem::Separator);
        items.push(item(
            "Quit".into(),
            MenuGlyph::Quit,
            self.tx.clone(),
            TrayCmd::Quit,
        ));
        items
    }
}

#[derive(Clone, Copy)]
enum MenuGlyph {
    Open,
    Pause,
    Play,
    Stop,
    Quit,
}

fn menu_icon_png(glyph: MenuGlyph) -> Vec<u8> {
    const OUT: u32 = 32;
    const SS: u32 = 4;
    const S: u32 = OUT * SS;
    let s = S as i32;
    let mut mask = vec![0u8; (S * S) as usize];

    let plot = |mask: &mut [u8], x: i32, y: i32| {
        if x >= 0 && y >= 0 && x < s && y < s {
            mask[(y * s + x) as usize] = 1;
        }
    };
    let fill_rect = |mask: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32| {
        for y in y0.max(0)..y1.min(s) {
            for x in x0.max(0)..x1.min(s) {
                mask[(y * s + x) as usize] = 1;
            }
        }
    };
    let fill_triangle = |mask: &mut [u8], ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32| {
        let area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if area.abs() < 1e-6 {
            return;
        }
        let minx = ax.min(bx).min(cx).floor() as i32;
        let maxx = ax.max(bx).max(cx).ceil() as i32;
        let miny = ay.min(by).min(cy).floor() as i32;
        let maxy = ay.max(by).max(cy).ceil() as i32;
        for y in miny.max(0)..maxy.min(s) {
            for x in minx.max(0)..maxx.min(s) {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = ((bx - ax) * (py - ay) - (by - ay) * (px - ax)) / area;
                let w1 = ((cx - bx) * (py - by) - (cy - by) * (px - bx)) / area;
                let w2 = ((ax - cx) * (py - cy) - (ay - cy) * (px - cx)) / area;
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    plot(mask, x, y);
                }
            }
        }
    };
    let stroke_line = |mask: &mut [u8], x0: f32, y0: f32, x1: f32, y1: f32, width: f32| {
        let r = width * 0.5;
        let minx = (x0.min(x1) - r).floor() as i32;
        let maxx = (x0.max(x1) + r).ceil() as i32;
        let miny = (y0.min(y1) - r).floor() as i32;
        let maxy = (y0.max(y1) + r).ceil() as i32;
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len2 = dx * dx + dy * dy;
        for y in miny.max(0)..maxy.min(s) {
            for x in minx.max(0)..maxx.min(s) {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let t = if len2 < 1e-6 {
                    0.0
                } else {
                    ((px - x0) * dx + (py - y0) * dy) / len2
                }
                .clamp(0.0, 1.0);
                let qx = x0 + t * dx - px;
                let qy = y0 + t * dy - py;
                if qx * qx + qy * qy <= r * r {
                    plot(mask, x, y);
                }
            }
        }
    };

    match glyph {
        MenuGlyph::Open => {
            fill_rect(&mut mask, 16, 40, 112, 112);
            fill_rect(&mut mask, 16, 32, 64, 56);
        }
        MenuGlyph::Pause => {
            fill_rect(&mut mask, 32, 24, 56, 104);
            fill_rect(&mut mask, 72, 24, 96, 104);
        }
        MenuGlyph::Play => {
            fill_triangle(&mut mask, 40.0, 24.0, 40.0, 104.0, 104.0, 64.0);
        }
        MenuGlyph::Stop => {
            fill_rect(&mut mask, 32, 32, 96, 96);
        }
        MenuGlyph::Quit => {
            stroke_line(&mut mask, 32.0, 32.0, 96.0, 96.0, 16.0);
            stroke_line(&mut mask, 96.0, 32.0, 32.0, 96.0, 16.0);
        }
    }

    const C: u8 = 230;
    let mut rgba = vec![0u8; (OUT * OUT * 4) as usize];
    let samples = SS * SS;
    for oy in 0..OUT {
        for ox in 0..OUT {
            let mut sum = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let i = ((oy * SS + sy) * S + ox * SS + sx) as usize;
                    sum += mask[i] as u32;
                }
            }
            let a = ((sum * 255) / samples) as u8;
            if a == 0 {
                continue;
            }
            let i = ((oy * OUT + ox) * 4) as usize;
            rgba[i] = C;
            rgba[i + 1] = C;
            rgba[i + 2] = C;
            rgba[i + 3] = a;
        }
    }

    let mut png = Vec::new();
    {
        let encoder = image::codecs::png::PngEncoder::new(&mut png);
        let _ = image::ImageEncoder::write_image(
            encoder,
            &rgba,
            OUT,
            OUT,
            image::ExtendedColorType::Rgba8,
        );
    }
    png
}

fn make_tray_icon(size: i32) -> ksni::Icon {
    let rgba = nwall_ipc::nwall_icon_rgba(size as u32);
    let mut data = vec![0u8; rgba.len()];
    for i in 0..(size * size) as usize {
        let s = i * 4;
        data[s] = rgba[s + 3];
        data[s + 1] = rgba[s];
        data[s + 2] = rgba[s + 1];
        data[s + 3] = rgba[s + 2];
    }
    ksni::Icon {
        width: size,
        height: size,
        data,
    }
}

fn icon_dir() -> std::path::PathBuf {
    let mut dir = if let Some(c) = std::env::var_os("XDG_DATA_HOME") {
        std::path::PathBuf::from(c)
    } else {
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
            .join(".local/share")
    };
    dir.push("nwall");
    dir.push("icons");
    dir
}

fn install_tray_icon() {
    let dir = icon_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("nwall.png");
    let rgba = nwall_ipc::nwall_icon_rgba(32);
    let _ = image::save_buffer(
        &path,
        &rgba,
        32,
        32,
        image::ExtendedColorType::Rgba8,
    );
    if let Some(home) = std::env::var_os("HOME") {
        let hicolor = std::path::PathBuf::from(home)
            .join(".local/share/icons/hicolor/32x32/apps");
        let _ = std::fs::create_dir_all(&hicolor);
        let _ = std::fs::copy(&path, hicolor.join("nwall.png"));
    }
}

pub fn spawn(tx: Sender<TrayCmd>) -> TrayHandle {
    install_tray_icon();
    let service = ksni::TrayService::new(NwallTray {
        tx,
        slideshow: false,
        show_slideshow: true,
        show_gui: gui_installed(),
        video: false,
        paused: false,
        bg_music: false,
        bg_music_mute: false,
        quitting: false,
    });
    let handle = TrayHandle(service.handle());
    service.spawn();
    handle
}

pub fn open_gui() {
    let _ = Command::new("nwall-gui").spawn();
}

pub fn gui_installed() -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            for dir in std::env::split_paths(&paths) {
                let p = dir.join("nwall-gui");
                if p.is_file() {
                    return true;
                }
            }
            false
        })
        .unwrap_or(false)
}
