// kei_ui — aris-render HTML browser UI for kei OS.
// Uses Blitz DOM + Vello cpu to render HTML, then writes to /dev/fb0.
// Avoids tracing-subscriber init (musl hang) — uses libc::write instead.

// Test if .init_array constructors are executed on kei.
#[cfg(unix)]
static mut CTOR_RAN: u32 = 0;

// Register a function pointer in .init_array section
#[cfg(unix)]
#[unsafe(link_section = ".init_array")]
#[used]
static CTOR: unsafe extern "C" fn() = unsafe { ctor_init };

#[cfg(unix)]
unsafe extern "C" fn ctor_init() {
    unsafe {
        CTOR_RAN = 0xDEAD_BEEF;
    }
}

fn main() {
    real_main();
}

#[cfg(not(unix))]
fn real_main() {
    eprintln!("kei_ui: unix-only binary (kei /dev/fb0 target); nothing to do on this host");
}

#[cfg(unix)]
fn real_main() {
    // Check if constructor ran
    let ctor_ran = unsafe { CTOR_RAN };
    let msg: &[u8] = if ctor_ran == 0xDEAD_BEEF {
        b"kei_ui: ctor OK\n"
    } else {
        b"kei_ui: ctor MISSING\n"
    };
    unsafe {
        libc::write(2, msg.as_ptr() as *const _, msg.len() as _);
    }
    let msg = b"kei_ui: starting\n";
    unsafe {
        libc::write(2, msg.as_ptr() as *const _, msg.len() as _);
    }

    let html = r#"<!DOCTYPE html><html><body style="background:#282C34"><div style="background:#61AFEF;width:100%;height:60px"></div></body></html>"#;

    let config = aris_render::RenderConfig {
        width: 640,
        height: 480,
        scale: 1.0,
    };

    let msg = b"kei_ui: rendering HTML\n";
    unsafe {
        libc::write(2, msg.as_ptr() as *const _, msg.len() as _);
    }

    let frame = match aris_render::render_html(html, &config) {
        Ok(f) => f,
        Err(e) => {
            let m = format!("kei_ui: render error: {:?}\n", e);
            unsafe {
                libc::write(2, m.as_ptr() as *const _, m.len() as _);
            }
            std::process::exit(1);
        }
    };

    let total = (frame.width as usize) * (frame.height as usize);
    let non_black = frame
        .rgba
        .chunks_exact(4)
        .filter(|px| px[0] > 10 || px[1] > 10 || px[2] > 10)
        .count();
    let msg = format!(
        "kei_ui: rendered {}/{} non-black ({:.1}%)\n",
        non_black,
        total,
        100.0 * non_black as f64 / total as f64
    );
    unsafe {
        libc::write(2, msg.as_ptr() as *const _, msg.len() as _);
    }

    // Write to /dev/fb0 row-by-row (avoids large write hang)
    #[cfg(unix)]
    {
        let fb_path = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "/dev/fb0".to_string());
        if std::path::Path::new(&fb_path).exists() {
            let msg = b"kei_ui: writing to fb0\n";
            unsafe {
                libc::write(2, msg.as_ptr() as *const _, msg.len() as _);
            }

            if let Ok(mut fb) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&fb_path)
            {
                use std::io::{Seek, Write};
                let row_bytes = frame.width as usize * 4;
                for y in 0..frame.height as usize {
                    let _ = fb.seek(std::io::SeekFrom::Start((y * row_bytes) as u64));
                    // Convert RGBA row to BGRX
                    let mut bgrx_row = vec![0u8; row_bytes];
                    for x in 0..frame.width as usize {
                        let src = (y * frame.width as usize + x) * 4;
                        let dst = x * 4;
                        bgrx_row[dst] = frame.rgba[src + 2]; // B
                        bgrx_row[dst + 1] = frame.rgba[src + 1]; // G
                        bgrx_row[dst + 2] = frame.rgba[src]; // R
                        bgrx_row[dst + 3] = 0xFF; // X
                    }
                    let _ = fb.write_all(&bgrx_row);
                }
                let msg = b"kei_ui: fb write done\n";
                unsafe {
                    libc::write(2, msg.as_ptr() as *const _, msg.len() as _);
                }
            }
        }
    }

    let msg = b"kei_ui: UI active\n";
    unsafe {
        libc::write(2, msg.as_ptr() as *const _, msg.len() as _);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
