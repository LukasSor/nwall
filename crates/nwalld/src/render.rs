
use anyhow::{Context, Result};
use fast_image_resize::images::{Image as FirImage, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::RgbaImage;
use nwall_ipc::FitMode;
use std::path::Path;

/// Cap still edge so huge files stay attachable without huge SHM.
const MAX_IMAGE_EDGE: u32 = 3840;

pub fn load_rgba(path: &Path) -> Result<RgbaImage> {
    let img = image::open(path)
        .with_context(|| format!("open image {}", path.display()))?
        .to_rgba8();
    cap_max_edge(img, MAX_IMAGE_EDGE)
}

fn cap_max_edge(img: RgbaImage, max_edge: u32) -> Result<RgbaImage> {
    let sw = img.width();
    let sh = img.height();
    let m = sw.max(sh);
    if m <= max_edge {
        return Ok(img);
    }
    let scale = max_edge as f64 / m as f64;
    let dw = ((sw as f64 * scale).round() as u32).max(1);
    let dh = ((sh as f64 * scale).round() as u32).max(1);
    log::info!("image {sw}x{sh} capped to {dw}x{dh}");
    let bytes = scale_fit(&img, dw, dh, FitMode::Stretch)?;
    RgbaImage::from_raw(dw, dh, bytes).ok_or_else(|| anyhow::anyhow!("capped rgba"))
}

/// Scale `src` into `dst_w` x `dst_h` according to fit mode. Returns tightly packed RGBA8.
pub fn scale_fit(src: &RgbaImage, dst_w: u32, dst_h: u32, fit: FitMode) -> Result<Vec<u8>> {
    let sw = src.width();
    let sh = src.height();
    if dst_w == 0 || dst_h == 0 {
        return Ok(Vec::new());
    }

    let (tw, th, dx, dy) = match fit {
        FitMode::Stretch => (dst_w, dst_h, 0u32, 0u32),
        FitMode::Cover => {
            let scale = (dst_w as f64 / sw as f64).max(dst_h as f64 / sh as f64);
            let tw = (sw as f64 * scale).round().max(1.0) as u32;
            let th = (sh as f64 * scale).round().max(1.0) as u32;
            let dx = tw.saturating_sub(dst_w) / 2;
            let dy = th.saturating_sub(dst_h) / 2;
            (tw, th, dx, dy)
        }
        FitMode::Contain => {
            let scale = (dst_w as f64 / sw as f64).min(dst_h as f64 / sh as f64);
            let tw = (sw as f64 * scale).round().max(1.0) as u32;
            let th = (sh as f64 * scale).round().max(1.0) as u32;
            let dx = 0;
            let dy = 0;
            (tw, th, dx, dy)
        }
    };

    let src_ref = ImageRef::new(sw, sh, src.as_raw(), PixelType::U8x4)
        .map_err(|e| anyhow::anyhow!("ImageRef: {e}"))?;
    let mut resized = FirImage::new(tw, th, PixelType::U8x4);
    let mut resizer = Resizer::new();
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    resizer
        .resize(&src_ref, &mut resized, &opts)
        .map_err(|e| anyhow::anyhow!("resize: {e}"))?;

    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];
    match fit {
        FitMode::Stretch => {
            out.copy_from_slice(resized.buffer());
        }
        FitMode::Cover => {
            let buf = resized.buffer();
            for y in 0..dst_h {
                let sy = y + dy;
                let src_row = ((sy * tw + dx) * 4) as usize;
                let dst_row = (y * dst_w * 4) as usize;
                let len = (dst_w * 4) as usize;
                out[dst_row..dst_row + len].copy_from_slice(&buf[src_row..src_row + len]);
            }
        }
        FitMode::Contain => {
            // Letterbox on black.
            let ox = (dst_w.saturating_sub(tw)) / 2;
            let oy = (dst_h.saturating_sub(th)) / 2;
            let buf = resized.buffer();
            for y in 0..th {
                let src_row = (y * tw * 4) as usize;
                let dst_row = (((y + oy) * dst_w + ox) * 4) as usize;
                let len = (tw * 4) as usize;
                out[dst_row..dst_row + len].copy_from_slice(&buf[src_row..src_row + len]);
            }
        }
    }
    Ok(out)
}

/// Downscale + cheap box blur for overview backdrop (CPU, once per wallpaper change).
pub fn make_backdrop(src: &RgbaImage, max_edge: u32, blur_radius: u32) -> Result<RgbaImage> {
    let sw = src.width();
    let sh = src.height();
    let scale = (max_edge as f64 / sw.max(sh) as f64).min(1.0);
    let dw = ((sw as f64 * scale).round() as u32).max(1);
    let dh = ((sh as f64 * scale).round() as u32).max(1);
    let scaled = scale_fit(src, dw, dh, FitMode::Stretch)?;
    let mut img = RgbaImage::from_raw(dw, dh, scaled)
        .ok_or_else(|| anyhow::anyhow!("backdrop rgba"))?;
    if blur_radius > 0 {
        box_blur_inplace(&mut img, blur_radius);
    }
    Ok(img)
}

fn box_blur_inplace(img: &mut RgbaImage, radius: u32) {
    let r = radius.max(1);
    let w = img.width() as usize;
    let h = img.height() as usize;
    let src = img.as_raw().clone();
    let mut tmp = vec![0u8; src.len()];

    // Horizontal
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            let x0 = x.saturating_sub(r as usize);
            let x1 = (x + r as usize).min(w - 1);
            for xx in x0..=x1 {
                let i = (y * w + xx) * 4;
                for c in 0..4 {
                    sum[c] += src[i + c] as u32;
                }
                count += 1;
            }
            let o = (y * w + x) * 4;
            for c in 0..4 {
                tmp[o + c] = (sum[c] / count) as u8;
            }
        }
    }

    // Vertical
    let mut out = vec![0u8; src.len()];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            let y0 = y.saturating_sub(r as usize);
            let y1 = (y + r as usize).min(h - 1);
            for yy in y0..=y1 {
                let i = (yy * w + x) * 4;
                for c in 0..4 {
                    sum[c] += tmp[i + c] as u32;
                }
                count += 1;
            }
            let o = (y * w + x) * 4;
            for c in 0..4 {
                out[o + c] = (sum[c] / count) as u8;
            }
        }
    }
    img.as_mut().copy_from_slice(&out);
}

/// Convert tightly packed RGBA8 to wl_shm Xrgb8888 / Argb8888 little-endian words.
pub fn rgba_to_argb8888(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for chunk in rgba.chunks_exact(4) {
        let (r, g, b, a) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        out.extend_from_slice(&[b, g, r, a]); // wl_shm argb8888 is often little-endian XRGB with BGRA byte order
    }
    out
}
