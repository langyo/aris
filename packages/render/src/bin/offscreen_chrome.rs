// Offscreen chrome + page render test — NO window, NO mouse.
//
// Renders a fake browser frame (chrome bar + a simple page) directly into an
// RGBA buffer using the same draw_chrome / present logic the winit backend
// uses, scaled to a chosen window size. Writes a PNG so we can visually verify
// the toolbar icons (back/forward/reload), the address bar text, and the page
// width — without touching the user's display or input.
//
//   cargo run -p aris-render --features "desktop winit" --bin offscreen_chrome -- <width> <height> <out.png>

use std::num::NonZeroU32;

fn main() {
    let width: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(800);
    let height: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let out = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "offscreen_chrome.png".to_string());

    aris_render::init_logging();
    let scale = 1.0_f32; // simulate a 1.0 scale_factor (DPI)
    let chrome_h_css = 44.0_f32;
    let chrome_phys = (chrome_h_css * scale).round() as u32;
    let page_h = height.saturating_sub(chrome_phys);

    // Render a simple page via the aris pipeline at the chosen size.
    let html = "<!DOCTYPE html><html><head><style>\
        body{margin:0;background:#1a1b26;color:#a9b1d6;font-family:system-ui;padding:16px;}\
        h1{color:#7aa2f7;} .w{color:#9ece6a;}</style></head>\
        <body><h1>offscreen page</h1><p class=\"w\">width probe</p>\
        <p>This line should fit inside the window width.</p></body></html>";
    let config = aris_render::RenderConfig {
        width,
        height: page_h,
        scale,
    };
    let frame = aris_render::render_html(html, &config).expect("render");

    // Build the full window buffer (page + chrome), mirroring present().
    let mut buf = vec![0u32; (width as usize) * (height as usize)];
    // Copy page frame below the chrome bar.
    let xw = width as usize;
    for y in 0..(page_h as usize) {
        let row = (y + chrome_phys as usize) * xw;
        for x in 0..xw {
            let src = (y * xw + x) * 4;
            if src + 2 < frame.rgba.len() {
                let r = frame.rgba[src] as u32;
                let g = frame.rgba[src + 1] as u32;
                let b = frame.rgba[src + 2] as u32;
                buf[row + x] = (r << 16) | (g << 8) | b;
            }
        }
    }

    // Draw the chrome bar. We reach into the winit_backend's draw_chrome via a
    // thin shim — but that function is private to the winit feature. Instead we
    // re-exercise it through a tiny inline shim module path.
    // (draw_chrome is not pub, so we replicate the call by making it pub in lib.)
    let url = "https://example.com/some/long/path/here";
    aris_render::winit_backend::draw_chrome(
        &mut buf,
        xw,
        chrome_phys as usize,
        width as f32,
        scale,
        url,
        false, // address not focused
        false, // caret off
        true,  // can_back
        false, // can_forward
        None,  // no hover
    );

    // Optionally render a context menu overlay (4th arg = "menu").
    let render_menu = std::env::args().any(|a| a == "menu");
    if render_menu {
        let menu = aris_render::winit_backend::ContextMenu {
            x: 60.0,
            y: 60.0,
            items: vec![
                (
                    "Back".into(),
                    aris_render::winit_backend::ContextMenuAction::GoBack,
                    true,
                ),
                (
                    "Forward".into(),
                    aris_render::winit_backend::ContextMenuAction::GoForward,
                    false,
                ),
                (
                    "Reload".into(),
                    aris_render::winit_backend::ContextMenuAction::Reload,
                    true,
                ),
                (
                    "Copy page URL".into(),
                    aris_render::winit_backend::ContextMenuAction::CopyUrl,
                    true,
                ),
                (
                    "Edit address".into(),
                    aris_render::winit_backend::ContextMenuAction::FocusAddress,
                    true,
                ),
            ],
            hover: Some(0),
        };
        aris_render::winit_backend::draw_context_menu(&mut buf, xw, height as usize, scale, &menu);
    }

    // Encode to PNG (24bpp RGB for max compatibility).
    let _ = NonZeroU32::new(1);
    save_png_rgb(&buf, width, height, &out);
    println!("wrote {} ({}x{})", out, width, height);
}

fn save_png_rgb(buf: &[u32], w: u32, h: u32, path: &str) {
    use std::fs::File;
    use std::io::BufWriter;
    let file = File::create(path).expect("create png");
    let bw = &mut BufWriter::new(file);
    let mut encoder = png::Encoder::new(bw, w, h);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    let mut rgb = Vec::with_capacity(buf.len() * 3);
    for &px in buf {
        rgb.push(((px >> 16) & 0xFF) as u8);
        rgb.push(((px >> 8) & 0xFF) as u8);
        rgb.push((px & 0xFF) as u8);
    }
    writer.write_image_data(&rgb).expect("png write");
}
