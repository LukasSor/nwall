/// RGBA8 pixels for the tray / app icon (night sky, sun, two hills).
pub fn nwall_icon_rgba(size: u32) -> Vec<u8> {
    let s = size as usize;
    let mut data = vec![0u8; s * s * 4];
    let put = |data: &mut [u8], x: i32, y: i32, r: u8, g: u8, b: u8, a: u8| {
        if x < 0 || y < 0 || x >= size as i32 || y >= size as i32 {
            return;
        }
        let i = ((y as usize) * s + x as usize) * 4;
        data[i] = r;
        data[i + 1] = g;
        data[i + 2] = b;
        data[i + 3] = a;
    };
    let size_i = size as i32;
    let cx = (size_i - 1) as f32 / 2.0;
    let cy = cx;
    let rad = size as f32 * 0.48;
    for y in 0..size_i {
        for x in 0..size_i {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy > rad * rad {
                continue;
            }
            put(&mut data, x, y, 28, 32, 48, 255);
            let sx = size as f32 * 0.68;
            let sy = size as f32 * 0.32;
            let sdx = x as f32 - sx;
            let sdy = y as f32 - sy;
            if sdx * sdx + sdy * sdy < (size as f32 * 0.12).powi(2) {
                put(&mut data, x, y, 255, 196, 92, 255);
            }
            let h1 = size as f32 * 0.62 + (x as f32 - cx).abs() * 0.15;
            if y as f32 > h1 {
                put(&mut data, x, y, 62, 92, 140, 255);
            }
            let h2 = size as f32 * 0.78 - (x as f32 * 0.18);
            if y as f32 > h2 {
                put(&mut data, x, y, 46, 168, 130, 255);
            }
        }
    }
    data
}
