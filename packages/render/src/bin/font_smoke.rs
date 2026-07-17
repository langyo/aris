// font_smoke — offline font pipeline diagnostics + text rendering smoke test.
//
// Renders text-heavy HTML through the exact same path kei uses
// (embedded fonts, system_fonts: false) and writes a PNG for inspection.
// Also probes parley/fontique directly to report which families the
// embedded font collection actually matches.
//
// Usage:
//   cargo run -p aris-render --no-default-features --features render \
//       --bin font_smoke -- [out.png]

#[cfg(feature = "render")]
fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "font_smoke.png".to_string());

    // ── Stage 1: probe fontique registration + family matching ──────────
    use parley::fontique::{QueryFamily, QueryStatus};

    let mut font_ctx = aris_render::new_embedded_font_context();

    eprintln!("[smoke] families in collection:");
    for name in font_ctx.collection.family_names() {
        eprintln!("[smoke]   family: {}", name);
    }

    // Query generic families the way blitz/parley will.
    use parley::fontique::GenericFamily::*;
    let probes: Vec<QueryFamily> = vec![
        SansSerif.into(),
        Serif.into(),
        Monospace.into(),
        "DejaVu Sans".into(),
        "DejaVu Sans Mono".into(),
        "system-ui-nonexistent".into(),
    ];
    for probe in probes {
        let mut query = font_ctx
            .collection
            .query(&mut font_ctx.source_cache);
        query.set_families([probe]);
        let mut hits = 0usize;
        query.matches_with(|_font| {
            hits += 1;
            QueryStatus::Continue
        });
        eprintln!("[smoke] query {:?} → {} font hits", probe, hits);
    }

    // ── Stage 2: render text HTML via the kei render path ────────────────
    let html = r##"<!DOCTYPE html><html><head><style>
    body { margin:0; background:#000000; color:#d4d4d4;
           font-family:"DejaVu Sans"; font-size:20px; }
    .mono { font-family:monospace; font-size:18px; color:#9ece6a; }
    .default-font { color:#e0af68; }
    </style></head><body>
    <p>TEXT RENDER PROBE 0123456789</p>
    <p class="mono">[    0.000000] kei: monospace probe line</p>
    <p class="default-font">no font-family (UA default) probe</p>
    </body></html>"##;

    let config = aris_render::RenderConfig {
        width: 640,
        height: 220,
        scale: 1.0,
    };
    let frame = aris_render::render_html(html, &config).expect("render_html failed");

    let non_black = frame
        .rgba
        .chunks_exact(4)
        .filter(|px| px[0] > 16 || px[1] > 16 || px[2] > 16)
        .count();
    eprintln!(
        "[smoke] render_html: {}x{}, non-black px: {} ({:.2}%)",
        frame.width,
        frame.height,
        non_black,
        100.0 * non_black as f64 / (frame.width as f64 * frame.height as f64)
    );

    save_png(&out, frame.width, frame.height, &frame.rgba);
    eprintln!("[smoke] wrote {}", out);
}

#[cfg(feature = "render")]
fn save_png(path: &str, width: u32, height: u32, rgba: &[u8]) {
    use std::io::BufWriter;
    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

#[cfg(not(feature = "render"))]
fn main() {
    eprintln!("font_smoke requires --features render");
}
