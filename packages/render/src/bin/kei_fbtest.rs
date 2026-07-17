// kei_fbtest — aris-render browser UI via /dev/fb0 write path.
//
// This binary is part of the aris-render crate. It draws a browser-style
// desktop UI (blue header, address bar, info cards) by writing BGRX pixels
// to /dev/fb0, exercising the aris-render fbdev display pipeline on kei.
//
// IMPORTANT: avoids tracing-subscriber init (which triggers musl malloc init
// that hangs on kei). Uses raw libc::write for stderr output instead.
fn main() {
    real_main();
}

#[cfg(not(unix))]
fn real_main() {
    eprintln!("kei_fbtest: unix-only binary (kei /dev/fb0 target); nothing to do on this host");
}

#[cfg(unix)]
fn real_main() {
    // Skip aris_render::init_logging() — it pulls in tracing-subscriber which
    // initializes regex/malloc and hangs on kei's musl runtime. Use direct
    // write(2, ...) instead.
    let msg = b"kei_fbtest: starting browser UI\n";
    unsafe {
        libc::write(2, msg.as_ptr() as *const _, msg.len() as _);
    }

    #[cfg(unix)]
    {
        let fb_path = "/dev/fb0";
        if !std::path::Path::new(fb_path).exists() {
            let m = b"fb0 not found\n";
            unsafe {
                libc::write(2, m.as_ptr() as *const _, m.len() as _);
            }
            return;
        }

        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(fb_path)
        {
            Ok(f) => f,
            Err(_) => return,
        };

        let width = 640usize;
        let height = 480usize;
        let bpp = 4usize;
        let fb_size = width * height * bpp;

        let m = b"kei_fbtest: building UI\n";
        unsafe {
            libc::write(2, m.as_ptr() as *const _, m.len() as _);
        }

        let mut buf = vec![0u8; fb_size];

        // BGRX colors for kei virtio-gpu
        let header = [0xEFu8, 0xAF, 0x61, 0xFF]; // blue
        let bg = [0x34u8, 0x2C, 0x28, 0xFF]; // dark
        let card = [0x2Bu8, 0x25, 0x21, 0xFF]; // card bg
        let addrbg = [0x23u8, 0x1F, 0x1B, 0xFF]; // address bar
        let white = [0xFFu8, 0xFF, 0xFF, 0xFF];
        let green = [0x79u8, 0xC3, 0x98, 0xFF];
        let accent = [0x75u8, 0x6C, 0xE0, 0xFF];
        let text_c = [0xBFu8, 0xB2, 0xAB, 0xFF];

        for y in 0..height {
            for x in 0..width {
                let c = if y < 50 {
                    header
                } else if y >= 58 && y < 86 {
                    addrbg
                } else if (y >= 100 && y < 180) || (y >= 195 && y < 275) || (y >= 290 && y < 370) {
                    card
                } else {
                    bg
                };
                let idx = (y * width + x) * 4;
                buf[idx..idx + 4].copy_from_slice(&c);
            }
        }

        // Title text pattern (white dots)
        for x in 20..280 {
            for y in 18..38 {
                if (x % 10 < 5) && (y % 8 < 4) {
                    let idx = (y * width + x) * 4;
                    buf[idx..idx + 4].copy_from_slice(&white);
                }
            }
        }
        // Indicator lines
        let draw_line = |buf: &mut [u8], y: usize, x0: usize, x1: usize, c: [u8; 4]| {
            for x in x0..x1.min(width) {
                let idx = (y * width + x) * 4;
                buf[idx..idx + 4].copy_from_slice(&c);
            }
        };
        draw_line(&mut buf, 130, 30, 200, green);
        draw_line(&mut buf, 132, 30, 200, green);
        draw_line(&mut buf, 150, 30, 250, text_c);
        draw_line(&mut buf, 152, 30, 250, text_c);
        draw_line(&mut buf, 225, 30, 150, accent);
        draw_line(&mut buf, 227, 30, 150, accent);
        draw_line(&mut buf, 247, 30, 250, text_c);
        draw_line(&mut buf, 249, 30, 250, text_c);
        draw_line(&mut buf, 320, 30, 250, text_c);
        draw_line(&mut buf, 322, 30, 250, text_c);
        draw_line(&mut buf, 342, 30, 250, text_c);
        draw_line(&mut buf, 344, 30, 250, text_c);
        draw_line(&mut buf, 400, 20, 280, text_c);
        draw_line(&mut buf, 402, 20, 280, text_c);
        draw_line(&mut buf, 425, 20, 200, accent);
        draw_line(&mut buf, 427, 20, 200, accent);

        let m = b"kei_fbtest: writing rows to fb0\n";
        unsafe {
            libc::write(2, m.as_ptr() as *const _, m.len() as _);
        }

        // Write row-by-row (each row = width*4 bytes) to avoid large single
        // write() calls that hang in the kernel fb write_at path.
        use std::io::{Seek, Write};
        let row_bytes = width * bpp;
        for y in 0..height {
            let _ = file.seek(std::io::SeekFrom::Start((y * row_bytes) as u64));
            let row_start = y * row_bytes;
            let _ = file.write_all(&buf[row_start..row_start + row_bytes]);
        }

        let m = b"kei_fbtest: done\n";
        unsafe {
            libc::write(2, m.as_ptr() as *const _, m.len() as _);
        }
    }

    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
