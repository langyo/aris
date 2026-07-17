// prerender_vtty — pre-render the kei vtty console frame on the HOST.
//
// The kei vtty (kei_tty) shows a static status console: modern Linux kernel
// console style — black background, light monospace text, kernel-log layout
// (`[    0.000000] kei: ...`). QEMU TCG is too slow to run the Blitz + Vello
// CPU pipeline at boot on kei, so this binary runs the SAME aris-render
// pipeline offline on the host and bakes the result into a compact PNG that
// kei_tty embeds via include_bytes! and blits to /dev/fb0.
//
// Usage:
//   cargo run -p aris-render --no-default-features --features render \
//       --bin prerender_vtty -- [out.png] [width] [height]
//
// Default output: packages/render/assets/vtty_console.png (tracked in git —
// re-run this tool and commit the result whenever the console style changes).

#[cfg(feature = "render")]
fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "packages/render/assets/vtty_console.png".to_string());
    let width: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1280);
    let height: u32 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(800);

    let html = vtty_console_html();
    let config = aris_render::RenderConfig {
        width,
        height,
        scale: 1.0,
    };
    let frame = aris_render::render_html(&html, &config).expect("vtty render failed");

    save_png(&out, frame.width, frame.height, &frame.rgba);

    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "[prerender_vtty] {}x{} → {} ({} bytes)",
        frame.width, frame.height, out, size
    );
}

/// The kei vtty console document: Linux kernel boot-log style status console.
/// Monospace only, black background, light gray text — deliberately plain,
/// no sci-fi styling.
#[cfg(feature = "render")]
fn vtty_console_html() -> String {
    // (timestamp, subsystem, message) — timestamps follow kernel printk format.
    let lines: &[(&str, &str, &str)] = &[
        ("    0.000000", "kei", "booting kernel 0.1.0 (asterinas fork) on qemu-virt aarch64"),
        ("    0.000000", "kei", "machine: linux,dummy-virt — cortex-a72, 1 cpu, 2048 MiB"),
        ("    0.000214", "ostd", "memory: buddy allocator online, 2048 MiB managed"),
        ("    0.000391", "ostd", "trap: exception vectors installed (VBAR_EL1)"),
        ("    0.000522", "gicv3", "distributor + redistributor online, 256 INTIDs"),
        ("    0.000846", "timer", "arch timer PPI 30 armed @ 62.5 MHz"),
        ("    0.001208", "virtio-mmio", "probing 8 device slots"),
        ("    0.001355", "virtio-net", "eth0 up — 10.0.2.15/24 gw 10.0.2.2"),
        ("    0.001402", "virtio-gpu", "scanout 0 → 1280x800, registered as /dev/fb0"),
        ("    0.002108", "smp", "1 processor online (BSP)"),
        ("    0.002544", "initramfs", "unpacked cpio archive, /init found"),
        ("    0.003012", "kei", "syscall table ready — 318 syscalls wired"),
        ("    0.003290", "kei", "gateway mode — ws json-rpc listening on 0.0.0.0:8423"),
        ("    0.003311", "vtty", "aris-render console active on /dev/fb0 (1280x800)"),
        ("    0.003402", "kei", "boot complete in 3.40 ms"),
    ];

    let mut body = String::new();
    for (ts, subsys, msg) in lines {
        body.push_str(&format!(
            "<div class=\"line\"><span class=\"ts\">[{}]</span> <span class=\"sub\">{}:</span> {}</div>\n",
            ts, subsys, msg
        ));
    }

    format!(
        r##"<!DOCTYPE html><html><head><style>
* {{ margin:0; padding:0; }}
html, body {{ background:#000000; }}
body {{ padding:20px 24px; color:#d6d6d6;
        font-family:"DejaVu Sans Mono", monospace; font-size:16px; }}
.line {{ line-height:1.5; }}
.ts {{ color:#6e6e6e; }}
.sub {{ color:#a8a8a8; }}
.dim {{ color:#8a8a8a; }}
.gap {{ height:20px; }}
.prompt {{ color:#e8e8e8; }}
.cursor {{ color:#e8e8e8; }}
</style></head><body>
{}
<div class="gap"></div>
<div class="line dim">kei tty0 — gateway mode (headless HMI via ws json-rpc :8423)</div>
<div class="gap"></div>
<div class="line prompt">root@kei:~# <span class="cursor">&#9608;</span></div>
</body></html>"##,
        body
    )
}

#[cfg(feature = "render")]
fn save_png(path: &str, width: u32, height: u32, rgba: &[u8]) {
    use std::io::BufWriter;
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).expect("create output dir");
        }
    }
    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

#[cfg(not(feature = "render"))]
fn main() {
    eprintln!("prerender_vtty requires --features render");
}
