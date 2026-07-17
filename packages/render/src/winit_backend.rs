// SPDX-License-Identifier: BUSL-1.1

//! Desktop browser window backend using winit + softbuffer.
//!
//! Features:
//! - **HiDPI**: renders at physical pixel resolution (logical × scale_factor)
//! - **Browser chrome**: a 44px toolbar with back/forward/reload buttons and an
//!   editable address bar, drawn directly into the softbuffer surface above the
//!   page content.
//! - **Navigation**: clicking `<a href>` and submitting forms works via a
//!   [`BrowserNavigationProvider`](crate::browser::BrowserNavigationProvider);
//!   the address bar accepts URLs, file paths, or search queries.
//! - **Networking**: an [`HttpNetProvider`](crate::browser::HttpNetProvider)
//!   fetches `<img>`, `<link rel=stylesheet>`, and `@font-face` over HTTP(S)
//!   or `file://`.
//! - **Text input**: typing into `<input>`/`<textarea>` is driven by blitz-dom's
//!   own editor via `UiEvent::KeyDown` — no hand-rolled editing.
//! - **Instant hover**: overlay-based highlight on the cached base frame.
//! - **Single-instance**: kills previous aris_browser windows on startup.

#![cfg(feature = "winit")]

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::RenderConfig;
use crate::browser::{
    BrowserNavigationProvider, BrowserShellProvider, BrowserState, HttpNetProvider,
};

use blitz_dom::Document;
use blitz_html::HtmlDocument;
use blitz_traits::events::{
    BlitzPointerEvent, BlitzPointerId, KeyState, MouseEventButton, MouseEventButtons,
    PointerCoords, UiEvent,
};
use blitz_traits::shell::Viewport;
use url::Url;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon as WinitCursorIcon, Window, WindowId};

/// Height of the tab strip above the chrome bar, in CSS logical px.
const TAB_STRIP_HEIGHT_CSS: f32 = 28.0;
/// Height of the chrome bar (address bar + buttons), below the tab strip.
const CHROME_BAR_HEIGHT_CSS: f32 = 44.0;
/// Total top offset (tab strip + chrome bar) — the Y where page content begins.
const CHROME_HEIGHT_CSS: f32 = TAB_STRIP_HEIGHT_CSS + CHROME_BAR_HEIGHT_CSS;

/// Run a blocking event loop that renders `html` into a desktop window.
pub fn run_window(html: &str, config: &RenderConfig) -> anyhow::Result<()> {
    run_window_impl(html, None, false, config)
}

/// Load and render `url` (a URL or local path). Used by the CLI.
pub fn run_window_url(url: &str, config: &RenderConfig) -> anyhow::Result<()> {
    let normalized = crate::browser::normalize_input_to_url(url);
    let (html, final_url, fetch_remote) = match normalized.scheme() {
        "file" => {
            let path = normalized
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("invalid file path: {}", url))?;
            let html = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path.display(), e))?;
            (html, normalized, false)
        }
        "about" | "data" => (
            crate::browser::about_or_data_html(&normalized),
            normalized,
            false,
        ),
        _ => (loading_page(normalized.as_ref()), normalized, true),
    };
    run_window_impl(&html, Some(final_url), fetch_remote, config)
}

fn run_window_impl(
    initial_html: &str,
    initial_url: Option<Url>,
    fetch_remote: bool,
    config: &RenderConfig,
) -> anyhow::Result<()> {
    // Kill any previous aris_browser processes (avoid window pile-up).
    #[cfg(target_os = "windows")]
    {
        let our_pid = std::process::id();
        let _ = std::process::Command::new("powershell")
            .args([
                "-NoProfile", "-Command",
                &format!(
                    "Get-Process aris_browser -ErrorAction SilentlyContinue | Where-Object {{ $_.Id -ne {} }} | Stop-Process -Force",
                    our_pid
                ),
            ])
            .output();
    }

    let event_loop = EventLoop::new()?;
    let state = Arc::new(BrowserState::new());
    // For an http(s) initial URL, kick off the fetch immediately so the
    // loading page is replaced as soon as bytes arrive.
    if fetch_remote && let Some(url) = &initial_url {
        state.navigate_input(url.as_str());
    }
    // Create the initial tab.
    let first_tab = Tab {
        state: Arc::clone(&state),
        doc: None,
        current_html: initial_html.to_string(),
        current_url: initial_url.clone(),
        base_xrgb: Vec::new(),
        needs_rerender: false,
        #[cfg(feature = "js")]
        js: None,
        find: None,
        zoom: 1.0,
        title: "New Tab".to_string(),
        favicon: None,
    };
    let mut app = App {
        config: config.clone(),
        window: None,
        context: None,
        surface: None,
        tabs: vec![first_tab],
        active: 0,
        phys_size: (0, 0),
        scale_factor: 1.0,
        last_caret: Instant::now(),
        prev_cursor: WinitCursorIcon::Default,
        chrome: ChromeState::new(),
        last_mouse: (0.0, 0.0),
        modifiers: ModifiersState::default(),
        context_menu: None,
        should_quit: false,
        scrollbar_drag: None,
        favicon_inbox: Arc::new(Mutex::new(Vec::new())),
    };
    // If no explicit URL was given, try restoring the last session.
    if initial_url.is_none() {
        let saved = state.load_session();
        if let Some(last_url) = saved.last() {
            // Restore by navigating to the last URL — history is already set.
            state.navigate_input(last_url.as_str());
        }
    }
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Per-tab state: document, history, URL, JS runtime, etc.
struct Tab {
    state: Arc<BrowserState>,
    doc: Option<HtmlDocument>,
    /// Source HTML of the current document (kept so a resize rebuild keeps it).
    current_html: String,
    /// Canonical URL of the current document (base URL for relative resolution).
    current_url: Option<Url>,
    /// Pre-converted XRGB8888 of the rendered page (no chrome overlay).
    base_xrgb: Vec<u32>,
    needs_rerender: bool,
    /// Persistent JS runtime for <script> execution + addEventListener.
    #[cfg(feature = "js")]
    js: Option<crate::js_runtime::JsRuntime>,
    /// Ctrl+F find-in-page state, when the find bar is open.
    find: Option<FindState>,
    /// Page zoom level (1.0 = 100%).
    zoom: f32,
    /// Tab title (for the tab strip), from the page <title> or URL.
    title: String,
    /// Cached decoded favicon (RGBA8 + dimensions), fetched async.
    favicon: Option<Favicon>,
}

/// A decoded favicon image.
#[derive(Clone)]
struct Favicon {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// The browser application state, held across the event loop.
struct App {
    config: RenderConfig,
    window: Option<Rc<Window>>,
    context: Option<softbuffer::Context<Rc<Window>>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    /// All open tabs. The active tab is `tabs[active]`.
    tabs: Vec<Tab>,
    active: usize,
    phys_size: (u32, u32),
    scale_factor: f64,
    last_caret: Instant,
    prev_cursor: WinitCursorIcon,
    chrome: ChromeState,
    last_mouse: (f32, f32),
    modifiers: ModifiersState,
    /// Right-click context menu overlay, when open.
    context_menu: Option<ContextMenu>,
    /// Set by the close button; checked in about_to_wait to exit the loop.
    should_quit: bool,
    /// When dragging the scrollbar, the Y where the drag started (CSS px).
    scrollbar_drag: Option<f32>,
    /// Inbox for favicons fetched by background threads: (tab_index, favicon).
    favicon_inbox: Arc<Mutex<Vec<(usize, Favicon)>>>,
}

impl App {
    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// Create a new tab with the start page (or navigate to `url` if given).
    fn new_tab(&mut self, url: Option<&str>) {
        let state = Arc::new(BrowserState::new());
        let html = start_page();
        let tab = Tab {
            state: Arc::clone(&state),
            doc: None,
            current_html: html,
            current_url: None,
            base_xrgb: Vec::new(),
            needs_rerender: false,
            #[cfg(feature = "js")]
            js: None,
            find: None,
            zoom: 1.0,
            title: "New Tab".to_string(),
            favicon: None,
        };
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        // Sync the address bar + build the doc.
        self.chrome.address.clear();
        self.chrome.address_focused = false;
        self.build_doc();
        if let Some(u) = url {
            self.active_tab_mut().state.navigate_input(u);
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Close the active tab. If it was the last tab, quit. Otherwise switch to
    /// the adjacent tab.
    fn close_tab(&mut self) {
        if self.tabs.len() <= 1 {
            self.should_quit = true;
            return;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        // Sync chrome for the new active tab.
        let url = self
            .active_tab()
            .current_url
            .as_ref()
            .map(|u| u.to_string())
            .unwrap_or_default();
        self.chrome.set_url(&url);
        self.chrome.address_focused = false;
        self.build_doc();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Switch to the next/previous tab (wraps around).
    fn switch_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n <= 1 {
            return;
        }
        if forward {
            self.active = (self.active + 1) % n;
        } else if self.active == 0 {
            self.active = n - 1;
        } else {
            self.active -= 1;
        }
        // Sync chrome for the new active tab.
        let url = self
            .active_tab()
            .current_url
            .as_ref()
            .map(|u| u.to_string())
            .unwrap_or_default();
        self.chrome.set_url(&url);
        self.chrome.address_focused = false;
        self.active_tab_mut().find = None; // close any find bar from prev tab
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

/// State for the find-in-page overlay.
#[derive(Clone)]
struct FindState {
    /// Current query text typed in the find bar.
    query: String,
    /// Index of the active match (for next/prev cycling).
    active: usize,
    /// Cached match rectangles (page CSS px) for the current query.
    matches: Vec<(f32, f32, f32, f32)>,
    /// Caret blink for the find input.
    caret_blink: bool,
}

/// Build a FontContext that registers the embedded DejaVu fonts as a
/// guaranteed fallback, while still allowing blitz-dom's system_fonts
/// discovery to add platform fonts (DirectWrite on Windows, fontconfig
/// on Linux, CoreText on macOS).
///
/// The embedded faces are APPENDED to the generic-family and script-fallback
/// maps (registered fonts are not added there automatically), so text still
/// renders on machines where system font discovery finds nothing.
fn new_font_context() -> parley::FontContext {
    use parley::fontique::{
        Blob, Collection, CollectionOptions, FallbackKey, GenericFamily, Script, SourceCache,
    };
    let mut font_ctx = parley::FontContext {
        source_cache: SourceCache::new_shared(),
        collection: Collection::new(CollectionOptions {
            shared: false,
            system_fonts: true,
        }),
    };
    let sans_ids: Vec<_> = font_ctx
        .collection
        .register_fonts(
            Blob::new(std::sync::Arc::new(crate::EMBEDDED_FONT.to_vec())),
            None,
        )
        .into_iter()
        .map(|(family_id, _)| family_id)
        .collect();
    let mono_ids: Vec<_> = font_ctx
        .collection
        .register_fonts(
            Blob::new(std::sync::Arc::new(crate::EMBEDDED_MONO_FONT.to_vec())),
            None,
        )
        .into_iter()
        .map(|(family_id, _)| family_id)
        .collect();

    use GenericFamily::*;
    for generic in [SansSerif, Serif, Cursive, Fantasy, SystemUi, UiSerif, UiSansSerif, UiRounded, Emoji, Math, FangSong] {
        font_ctx
            .collection
            .append_generic_families(generic, sans_ids.iter().copied());
    }
    for generic in [Monospace, UiMonospace] {
        font_ctx.collection.append_generic_families(
            generic,
            mono_ids.iter().chain(sans_ids.iter()).copied(),
        );
    }
    let all_ids = || sans_ids.iter().chain(mono_ids.iter()).copied();
    for tag in [*b"Latn", *b"Grek", *b"Cyrl", *b"Zyyy", *b"Zinh"] {
        font_ctx
            .collection
            .append_fallbacks(FallbackKey::new(Script::from_bytes(tag), None), all_ids());
    }
    font_ctx
}

impl App {
    /// Window size in CSS logical px.
    fn css_size(&self) -> (f32, f32) {
        (
            self.phys_size.0 as f32 / self.scale_factor as f32,
            self.phys_size.1 as f32 / self.scale_factor as f32,
        )
    }

    /// Build a fresh document from `current_html` / `current_url`.
    fn build_doc(&mut self) {
        let (pw, ph) = self.phys_size;
        if pw == 0 || ph == 0 {
            return;
        }
        let chrome_phys = (CHROME_HEIGHT_CSS * self.scale_factor as f32).round() as u32;
        let page_w = pw;
        let page_h = ph.saturating_sub(chrome_phys).max(1);
        let viewport = Viewport {
            window_size: (page_w, page_h),
            hidpi_scale: self.scale_factor as f32,
            zoom: self.active_tab().zoom,
            color_scheme: if is_os_dark_mode() {
                blitz_traits::shell::ColorScheme::Dark
            } else {
                blitz_traits::shell::ColorScheme::Light
            },
            ..Default::default()
        };

        let net = Arc::new(HttpNetProvider::new());
        let nav = Arc::new(BrowserNavigationProvider::new(Arc::clone(
            &self.active_tab().state,
        )));
        let shell = Arc::new(BrowserShellProvider::new(Arc::clone(
            &self.active_tab().state,
        )));

        let mut doc_config = blitz_dom::DocumentConfig {
            viewport: Some(viewport),
            net_provider: Some(net),
            navigation_provider: Some(nav),
            shell_provider: Some(shell),
            font_ctx: Some(new_font_context()),
            ..Default::default()
        };
        if let Some(url) = &self.active_tab().current_url {
            doc_config.base_url = Some(url.to_string());
        }

        // Pre-process inline <script> blocks via Boa when the `js` feature is
        // enabled. This is SSR-style: scripts run once at load time, and any
        // `document.write` output is spliced into the HTML before parsing.
        // Full interactive DOM↔JS binding (onclick etc.) is a larger project.
        let html_to_parse: String = if cfg!(feature = "js") {
            #[cfg(feature = "js")]
            {
                run_scripts_ssr(&self.active_tab().current_html)
            }
            #[cfg(not(feature = "js"))]
            {
                self.active_tab().current_html.clone()
            }
        } else {
            self.active_tab().current_html.clone()
        };

        let mut doc = HtmlDocument::from_html(&html_to_parse, doc_config);
        // Clamp the root to the viewport so wide content can't force the page
        // wider than the window (a common cause of "uses maximized width"
        // symptoms on HiDPI displays where layout scales unexpectedly).
        doc.add_user_agent_stylesheet("html,body{max-width:100vw;overflow-x:hidden;}");
        doc.resolve(0.0);

        // With the `js` feature, run inline <script> blocks through the
        // persistent JsRuntime so addEventListener registrations survive to be
        // fired on click. (run_scripts_ssr already handled document.write
        // injection above.)
        #[cfg(feature = "js")]
        {
            let scripts = aris_js::extract_scripts(&self.active_tab().current_html);
            let combined = scripts.join("\n;\n");
            let url = self
                .active_tab()
                .current_url
                .as_ref()
                .map(|u| u.to_string())
                .unwrap_or_default();
            let rt = self
                .active_tab_mut()
                .js
                .get_or_insert_with(crate::js_runtime::JsRuntime::new);
            rt.bind_and_run_with_url(&mut doc, &combined, &url);
        }

        self.active_tab_mut().doc = Some(doc);
        self.active_tab_mut().needs_rerender = true;
    }

    /// Rasterize the page to `base_xrgb` (XRGB8888 u32). Chrome drawn on top.
    fn render_base_frame(&mut self) {
        let (pw, ph) = self.phys_size;
        if pw == 0 || ph == 0 {
            return;
        }
        let chrome_phys = (CHROME_HEIGHT_CSS * self.scale_factor as f32).round() as u32;
        let page_h = ph.saturating_sub(chrome_phys).max(1);
        let scale = self.scale_factor;
        let Some(doc) = self.active_tab_mut().doc.as_mut() else {
            return;
        };
        doc.resolve(0.0);
        let mut frame = crate::Frame::new(pw, page_h);
        use anyrender::ImageRenderer;
        let mut renderer = anyrender_vello_cpu::VelloCpuImageRenderer::new(pw, page_h);
        renderer.render(
            |scene| {
                blitz_paint::paint_scene(scene, doc, scale, pw, page_h, 0, 0);
                // Composite canvas 2D scenes onto the page at each <canvas>
                // element's layout position.
                #[cfg(feature = "js")]
                composite_canvases(scene, doc, scale);
            },
            &mut frame.rgba,
        );

        // RGBA → XRGB u32.
        let pixel_count = frame.width as usize * frame.height as usize;
        let mut xrgb = Vec::with_capacity(pixel_count);
        for i in 0..pixel_count {
            let src = i * 4;
            if src + 2 < frame.rgba.len() {
                let r = frame.rgba[src] as u32;
                let g = frame.rgba[src + 1] as u32;
                let b = frame.rgba[src + 2] as u32;
                xrgb.push((r << 16) | (g << 8) | b);
            } else {
                xrgb.push(0);
            }
        }
        {
            let tab = self.active_tab_mut();
            tab.base_xrgb = xrgb;
            tab.needs_rerender = false;
        }
        let _ = std::env::var("ARIS_DUMP_FRAME").map(|path| {
            let _ = std::fs::write(&path, &frame.rgba);
        });
    }

    /// Hovered node rect in page CSS px.
    fn hover_rect_page_css(&self) -> Option<(f32, f32, f32, f32)> {
        let doc = self.active_tab().doc.as_ref()?;
        let hover_id = doc.get_hover_node_id()?;
        let node = doc.get_node(hover_id)?;
        let pos = node.absolute_position(0.0, 0.0);
        let w = node.final_layout.size.width;
        let h = node.final_layout.size.height;
        if w < 1.0 || h < 1.0 {
            return None;
        }
        Some((pos.x, pos.y, w, h))
    }

    /// The URL of the link currently under the cursor, if any. Walks up from
    /// the hovered node to the nearest `<a href>`, mirroring how browsers
    /// populate the status bar.
    fn hovered_link_url(&self) -> Option<String> {
        let doc = self.active_tab().doc.as_ref()?;
        let mut id = doc.get_hover_node_id()?;
        for _ in 0..16 {
            let node = doc.get_node(id)?;
            if let Some(href) = node.attr(blitz_dom::local_name!("href")) {
                // Resolve relative to the base URL for display.
                if let Some(base) = &self.active_tab().current_url
                    && let Ok(resolved) = base.join(href)
                {
                    return Some(resolved.to_string());
                }
                return Some(href.to_string());
            }
            id = node.parent?;
        }
        None
    }

    /// Blit page frame, draw chrome, draw hover overlay.
    fn present(&mut self) {
        let (pw, ph) = self.phys_size;
        if pw == 0 || ph == 0 {
            return;
        }
        let scale = self.scale_factor as f32;
        let chrome_phys = (CHROME_HEIGHT_CSS * scale).round() as usize;
        let hover_rect = self.hover_rect_page_css();
        let css_w = self.css_size().0;
        let can_back = self.active_tab().state.can_go_back();
        let can_fwd = self.active_tab().state.can_go_forward();
        let navigating = self.active_tab().state.navigating.load(Ordering::Relaxed);
        let url_display = if self.chrome.address_focused {
            self.chrome.address.clone()
        } else if let Some(link) = self.hovered_link_url() {
            // Standard browser behavior: hovering a link shows its URL in the
            // address bar / status area.
            link
        } else {
            self.active_tab()
                .current_url
                .as_ref()
                .map(|u| u.to_string())
                .unwrap_or_default()
        };
        // While a navigation is in flight, prefix the address with a loading
        // glyph so the user gets feedback that the page is loading.
        let url_display = if navigating && !self.chrome.address_focused {
            format!("⟳ {}", url_display)
        } else {
            url_display
        };
        let focused = self.chrome.address_focused;
        let caret_on = self.chrome.caret_visible();
        let hover_region = self.chrome.hover;
        let menu = self.context_menu.clone();
        let scrollbar = self.scrollbar_metrics();
        let find = self.active_tab().find.clone();
        let status_text = if self.active_tab().state.navigating.load(Ordering::Relaxed) {
            Some("Loading…".to_string())
        } else {
            self.hovered_link_url()
        };
        // Snapshot the active tab's xrgb before borrowing surface mutably.
        let xrgb_snapshot: Vec<u32> = self.active_tab().base_xrgb.clone();
        // Snapshot tab titles + active index for the tab strip.
        let tab_titles: Vec<String> = self
            .tabs
            .iter()
            .map(|t| {
                if t.title.is_empty() {
                    t.current_url
                        .as_ref()
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| "New Tab".to_string())
                } else {
                    t.title.clone()
                }
            })
            .collect();
        let active_tab_idx = self.active;
        // Snapshot the active tab's favicon (cloned to avoid borrow conflict).
        let favicon_opt: Option<Favicon> = self.active_tab().favicon.clone();

        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let _ = surface.resize(NonZeroU32::new(pw).unwrap(), NonZeroU32::new(ph).unwrap());
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let buf_len = buffer.len();
        let buf_w = pw as usize;
        let buf_h = ph as usize;
        let xrgb = &xrgb_snapshot;
        let page_h = buf_h.saturating_sub(chrome_phys);
        let xw = xrgb.len().checked_div(page_h).unwrap_or(0);

        // Copy page base frame below the chrome bar.
        if !xrgb.is_empty() && xw > 0 {
            let copy_n = buf_w.min(xw);
            for y in 0..page_h {
                let row = (y + chrome_phys) * buf_w;
                let srow = y * xw;
                if row + copy_n <= buf_len && srow + copy_n <= xrgb.len() {
                    buffer[row..row + copy_n].copy_from_slice(&xrgb[srow..srow + copy_n]);
                }
            }
        }

        // Chrome bar.
        draw_chrome(
            &mut buffer,
            buf_w,
            chrome_phys,
            css_w,
            scale,
            &url_display,
            focused,
            caret_on,
            can_back,
            can_fwd,
            hover_region,
        );

        // Overlay the real favicon (if fetched) on top of the colored fallback.
        if let Some(fav) = &favicon_opt {
            let layout = ChromeLayout::compute(css_w);
            let rect = layout.favicon;
            let fx = (rect.0 * scale) as usize;
            let fy = (rect.1 * scale) as usize;
            let fw = (rect.2 * scale) as usize;
            let fh = (rect.3 * scale) as usize;
            for ty in 0..fh {
                for tx in 0..fw {
                    let sx = (tx as u32 * fav.width) / fw as u32;
                    let sy = (ty as u32 * fav.height) / fh as u32;
                    let sidx = ((sy * fav.width + sx) * 4) as usize;
                    if sidx + 2 < fav.rgba.len() {
                        let r = fav.rgba[sidx] as u32;
                        let g = fav.rgba[sidx + 1] as u32;
                        let b = fav.rgba[sidx + 2] as u32;
                        plot(
                            &mut buffer,
                            buf_w,
                            fx + tx,
                            fy + ty,
                            (r << 16) | (g << 8) | b,
                        );
                    }
                }
            }
        }

        // Tab strip (drawn above the chrome bar).
        draw_tab_strip(
            &mut buffer,
            buf_w,
            scale,
            css_w,
            &tab_titles,
            active_tab_idx,
        );

        // Hover highlight (page space, offset by chrome).
        if let Some((cx, cy, cw, ch)) = hover_rect {
            let px = (cx * scale) as i32;
            let py = (cy * scale) as i32 + chrome_phys as i32;
            let pw_px = (cw * scale) as i32;
            let ph_px = (ch * scale) as i32;
            for ry in 0..ph_px {
                let y = py + ry;
                if y < 0 || y as usize >= buf_h {
                    continue;
                }
                let row = y as usize * buf_w;
                for rx in 0..pw_px {
                    let x = px + rx;
                    if x < 0 || x as usize >= buf_w {
                        continue;
                    }
                    let idx = row + x as usize;
                    if idx >= buf_len {
                        break;
                    }
                    let pixel = buffer[idx];
                    let r = (pixel >> 16) & 0xFF;
                    let g = (pixel >> 8) & 0xFF;
                    let b = pixel & 0xFF;
                    let on_border = ry < 2 || rx < 2 || ry >= ph_px - 2 || rx >= pw_px - 2;
                    if on_border {
                        buffer[idx] = 0x00FFFF00;
                    } else {
                        let nr = r + ((255 - r) * 22 / 100);
                        let ng = g + ((255 - g) * 22 / 100);
                        let nb = b + ((255 - b) * 22 / 100);
                        buffer[idx] = (nr << 16) | (ng << 8) | nb;
                    }
                }
            }
        }

        // Scrollbar for the document viewport, if the page is scrollable.
        if let Some(sb) = scrollbar {
            draw_scrollbar(&mut buffer, buf_w, buf_h, chrome_phys, scale, sb);
        }

        // Find-in-page: highlight matches + draw the find bar.
        if let Some(f) = &find {
            draw_find_overlays(&mut buffer, buf_w, buf_h, chrome_phys, scale, css_w, f);
        }

        // Bottom status bar: shows the hovered link URL or a loading message.
        if let Some(text) = &status_text {
            draw_status_bar(&mut buffer, buf_w, buf_h, scale, css_w, text);
        }

        // Context menu overlay (top-most).
        if let Some(menu) = &menu {
            draw_context_menu(&mut buffer, buf_w, buf_h, scale, menu);
        }

        let _ = buffer.present();
    }

    /// Compute scrollbar geometry (content height, scroll position) for the
    /// document's scrollable root, if the content overflows the viewport.
    fn scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        let doc = self.active_tab().doc.as_ref()?;
        // Find the body element (the usual scroll container).
        let body_id = doc.tree().iter().find_map(|(id, n)| {
            n.element_data()
                .filter(|e| format!("{:?}", e.name.local).contains("'body'"))
                .map(|_| id)
        })?;
        let body = doc.get_node(body_id)?;
        let chrome_phys = (CHROME_HEIGHT_CSS * self.scale_factor as f32).round();
        let viewport_h_css =
            (self.phys_size.1 as f32 / self.scale_factor as f32) - CHROME_HEIGHT_CSS;
        // Content height: the body's content box height.
        let content_h = body.final_layout.size.height;
        if content_h <= viewport_h_css + 1.0 {
            return None; // not scrollable
        }
        let scroll_y = body.scroll_offset.y as f32;
        Some(ScrollbarMetrics {
            content_h,
            viewport_h: viewport_h_css,
            scroll_y,
            _chrome_phys: chrome_phys,
        })
    }

    /// Search the document for `query`, returning match rectangles (page CSS
    /// px). Each text node whose content contains the query contributes one
    /// rectangle per occurrence, positioned at the parent element's layout box.
    /// (Per-character positioning would need text-shaping data; element-level
    /// highlighting is the pragmatic approximation browsers' Ctrl+F start from.)
    fn find_matches(&self, query: &str) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        let Some(doc) = self.active_tab().doc.as_ref() else {
            return out;
        };
        if query.is_empty() {
            return out;
        }
        let needle = query.to_lowercase();
        for (id, node) in doc.tree().iter() {
            if node.text_data().is_none() {
                continue;
            }
            let content = node.text_content().to_lowercase();
            if !content.contains(&needle) {
                continue;
            }
            // Use this text node's own layout box if available, else the parent's.
            let pos = node.absolute_position(0.0, 0.0);
            let w = node.final_layout.size.width;
            let h = node.final_layout.size.height;
            if w < 1.0 || h < 1.0 {
                continue;
            }
            // One highlight rect per occurrence, horizontally distributed.
            let count = content.matches(&needle).count();
            for i in 0..count {
                let slice = w / count.max(1) as f32;
                let x = pos.x + slice * i as f32;
                out.push((x, pos.y, slice, h));
            }
            let _ = id;
        }
        out
    }

    /// Open the find bar with an empty query.
    fn open_find(&mut self) {
        self.active_tab_mut().find = Some(FindState {
            query: String::new(),
            active: 0,
            matches: Vec::new(),
            caret_blink: true,
        });
    }

    /// Update the find query and refresh matches.
    fn update_find_query(&mut self, q: &str) {
        let matches = self.find_matches(q);
        let Some(f) = self.active_tab_mut().find.as_mut() else {
            return;
        };
        f.query = q.to_string();
        f.active = 0;
        f.caret_blink = true;
        f.matches = matches;
    }

    /// Cycle to the next/previous match.
    fn find_next(&mut self, forward: bool) {
        let Some(f) = self.active_tab_mut().find.as_mut() else {
            return;
        };
        if f.matches.is_empty() {
            return;
        }
        if forward {
            f.active = (f.active + 1) % f.matches.len();
        } else if f.active == 0 {
            f.active = f.matches.len() - 1;
        } else {
            f.active -= 1;
        }
    }

    fn update_cursor_icon(&mut self) {
        let over_chrome = self.last_mouse.1 < CHROME_HEIGHT_CSS;
        let over_address = over_chrome
            && self
                .chrome
                .region_at(self.last_mouse.0, self.last_mouse.1, self.css_size().0)
                == Some(ChromeRegion::Address);
        let icon = if over_address || (!over_chrome && self.is_over_text()) {
            WinitCursorIcon::Text
        } else if over_chrome {
            let region =
                self.chrome
                    .region_at(self.last_mouse.0, self.last_mouse.1, self.css_size().0);
            match region {
                Some(ChromeRegion::Address) => WinitCursorIcon::Text,
                Some(
                    ChromeRegion::Back
                    | ChromeRegion::Forward
                    | ChromeRegion::Reload
                    | ChromeRegion::Close,
                ) => WinitCursorIcon::Pointer,
                None => WinitCursorIcon::Default,
            }
        } else {
            let cursor = self.active_tab().doc.as_ref().and_then(|d| d.get_cursor());
            match cursor {
                Some(c) => {
                    let name = format!("{:?}", c).to_lowercase();
                    match name.as_str() {
                        "pointer" => WinitCursorIcon::Pointer,
                        "text" => WinitCursorIcon::Text,
                        "wait" => WinitCursorIcon::Wait,
                        "crosshair" => WinitCursorIcon::Crosshair,
                        "notallowed" => WinitCursorIcon::NotAllowed,
                        "grab" => WinitCursorIcon::Grab,
                        "grabbing" => WinitCursorIcon::Grabbing,
                        "help" => WinitCursorIcon::Help,
                        "move" => WinitCursorIcon::AllScroll,
                        _ => WinitCursorIcon::Default,
                    }
                }
                None => WinitCursorIcon::Default,
            }
        };
        if icon != self.prev_cursor {
            if let Some(window) = &self.window {
                window.set_cursor(icon);
            }
            self.prev_cursor = icon;
        }
    }

    fn is_over_text(&self) -> bool {
        self.active_tab()
            .doc
            .as_ref()
            .and_then(|d| d.get_cursor())
            .map(|c| matches!(format!("{:?}", c).to_lowercase().as_str(), "text"))
            .unwrap_or(false)
    }

    /// Dispatch a pointer event to the document at page-space coords.
    fn dispatch_pointer(&mut self, css_x: f32, css_y: f32, pressed: bool) {
        let mods = winit_modifiers_to_blitz(self.modifiers);
        let pe = BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: PointerCoords {
                page_x: css_x,
                page_y: css_y,
                screen_x: css_x,
                screen_y: css_y,
                client_x: css_x,
                client_y: css_y,
            },
            button: MouseEventButton::Main,
            buttons: if pressed {
                MouseEventButtons::Primary
            } else {
                MouseEventButtons::empty()
            },
            mods,
            details: unsafe { core::mem::zeroed() },
            element: Default::default(),
        };
        let ui = if pressed {
            UiEvent::PointerDown(pe)
        } else {
            UiEvent::PointerUp(pe)
        };
        if let Some(doc) = self.active_tab_mut().doc.as_mut() {
            doc.handle_ui_event(ui);
        }
        // On pointer-up (the click), fire JS handlers. Done outside the doc
        // borrow above so the JS runtime can mutate the document.
        if !pressed {
            let target = self
                .active_tab()
                .doc
                .as_ref()
                .and_then(|d| d.get_hover_node_id())
                .unwrap_or(0);
            #[cfg(feature = "js")]
            {
                // onclick attribute + addEventListener listeners both go through
                // the persistent runtime.
                if let Some(tab) = self.tabs.get_mut(self.active)
                    && let (Some(rt), Some(doc)) = (tab.js.as_mut(), tab.doc.as_mut())
                {
                    // onclick attribute (inline handler): eval its source in the
                    // runtime so it shares the document bridge.
                    let onclick_src = doc
                        .get_node(target)
                        .and_then(|n| n.attr(blitz_dom::local_name!("onclick")))
                        .map(|s| s.to_string());
                    rt.bind_and_run(doc, onclick_src.as_deref().unwrap_or(""));
                    // Registered click listeners.
                    rt.fire_click(doc, target as u32);
                    self.active_tab_mut().needs_rerender = true;
                }
            }
            let _ = target;
        }
    }

    /// Process queued page loads from providers/fetch threads.
    fn process_loads(&mut self) {
        let loads = self.active_tab().state.drain_loads();
        if loads.is_empty() {
            return;
        }
        if let Some(load) = loads.into_iter().last() {
            tracing::info!("loading document: {}", load.url);
            self.active_tab_mut().current_html = load.html;
            self.active_tab_mut().current_url = Some(load.url.clone());
            self.chrome.set_url(load.url.as_ref());
            self.build_doc();
            self.active_tab().state.commit_load(load.url);
            // Try to fetch the favicon for this page.
            self.try_fetch_favicon();
        }
    }

    /// Fetch and decode the favicon for the active tab's current page, in a
    /// background thread. The decoded RGBA is pushed to `favicon_inbox` for
    /// the render loop to drain and attach to the tab.
    fn try_fetch_favicon(&mut self) {
        let base_url = match &self.active_tab().current_url {
            Some(u) => u.clone(),
            None => return,
        };
        let href = match self.active_tab().doc.as_ref().and_then(|d| d.favicon_url()) {
            Some(h) => h,
            None => return,
        };
        let fav_url = match base_url.join(&href) {
            Ok(u) => u,
            Err(_) => return,
        };
        if fav_url.scheme() != "http" && fav_url.scheme() != "https" {
            return;
        }
        let url_str = fav_url.to_string();
        let active = self.active;
        let inbox = Arc::clone(&self.favicon_inbox);
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .user_agent("aris/0.1 (like Gecko)")
                .build()
                .unwrap_or_else(|_| reqwest::blocking::Client::new());
            let Ok(resp) = client.get(&url_str).send() else {
                return;
            };
            if !resp.status().is_success() {
                return;
            }
            let Ok(bytes) = resp.bytes() else {
                return;
            };
            if let Ok(img) = image::load_from_memory(&bytes) {
                let rgba = img.to_rgba8();
                let fav = Favicon {
                    rgba: rgba.as_raw().to_vec(),
                    width: rgba.width(),
                    height: rgba.height(),
                };
                inbox.lock().unwrap().push((active, fav));
            }
        });
    }

    fn pump_messages(&mut self) {
        if let Some(doc) = self.active_tab_mut().doc.as_mut() {
            doc.handle_messages();
        }
    }

    /// Scroll the document viewport by one page (or a fraction for Space).
    /// `up` reverses direction; `page` = true scrolls a viewport height, false
    /// a third (used for Space without Shift).
    fn scroll_viewport(&mut self, up: bool, page: bool) {
        let dy = if page {
            let vh = self.phys_size.1 as f64 / self.scale_factor - CHROME_HEIGHT_CSS as f64;
            if up { vh } else { -vh }
        } else if up {
            120.0
        } else {
            -120.0
        };
        if let Some(doc) = self.active_tab_mut().doc.as_mut() {
            let hover = doc.get_hover_node_id().unwrap_or(0);
            doc.scroll_node_by(hover, dy, 0.0, &mut |_| {});
            self.active_tab_mut().needs_rerender = true;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    /// Scroll the document to an absolute y position (`0.0` = top).
    fn scroll_to(&mut self, _y: f64) {
        if let Some(doc) = self.active_tab_mut().doc.as_mut() {
            let hover = doc.get_hover_node_id().unwrap_or(0);
            // scroll_node_by is relative; for absolute we'd need the current
            // offset. As a pragmatic approximation, scroll by a large negative
            // delta to reach the bottom, or scroll to top via the viewport API.
            doc.scroll_viewport_by(0.0, -100000.0);
            self.active_tab_mut().needs_rerender = true;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            let _ = hover;
        }
    }

    /// Scroll the document so the scrollbar thumb matches a drag position.
    /// `drag_y` is the cursor Y in CSS px.
    fn scroll_to_drag(&mut self, drag_y: f32) {
        let Some(sb) = self.scrollbar_metrics() else {
            return;
        };
        let css_h = self.phys_size.1 as f32 / self.scale_factor as f32;
        let track_top = CHROME_HEIGHT_CSS;
        let track_h = (css_h - track_top).max(1.0);
        let frac = ((drag_y - track_top) / track_h).clamp(0.0, 1.0);
        let target = frac * (sb.content_h - sb.viewport_h).max(0.0);
        let delta = sb.scroll_y - target;
        if delta.abs() > 0.5
            && let Some(doc) = self.active_tab_mut().doc.as_mut()
        {
            let hover = doc.get_hover_node_id().unwrap_or(0);
            doc.scroll_node_by(hover, delta as f64, 0.0, &mut |_| {});
            self.active_tab_mut().needs_rerender = true;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn apply_title(&mut self) {
        if let Some(title) = self.active_tab().state.take_title()
            && let Some(window) = &self.window
        {
            let display = if title.trim().is_empty() {
                "aris".to_string()
            } else {
                format!("{} — aris", title)
            };
            window.set_title(&display);
        }
    }

    /// Handle a chrome-originated action.
    fn handle_chrome_action(&mut self, action: ChromeAction) {
        match action {
            ChromeAction::GoBack => {
                self.active_tab().state.go_back();
            }
            ChromeAction::GoForward => {
                self.active_tab().state.go_forward();
            }
            ChromeAction::Reload => {
                self.active_tab().state.reload();
            }
            ChromeAction::Navigate(input) => {
                if !input.trim().is_empty() {
                    self.active_tab().state.navigate_input(&input);
                }
            }
            ChromeAction::RedrawOnly => {}
            ChromeAction::CloseWindow => {
                self.should_quit = true;
            }
        }
    }

    /// Open a right-click context menu at (x, y), populated based on context.
    fn open_context_menu(&mut self, x: f32, y: f32) {
        let mut items: Vec<(String, ContextMenuAction, bool)> = Vec::new();
        let can_back = self.active_tab().state.can_go_back();
        let can_fwd = self.active_tab().state.can_go_forward();
        items.push(("Back".to_string(), ContextMenuAction::GoBack, can_back));
        items.push(("Forward".to_string(), ContextMenuAction::GoForward, can_fwd));
        items.push(("Reload".to_string(), ContextMenuAction::Reload, true));
        if self.hovered_link_url().is_some() {
            items.push((
                "Copy link address".to_string(),
                ContextMenuAction::CopyLink,
                true,
            ));
        }
        if self.active_tab().current_url.is_some() {
            items.push((
                "Copy page URL".to_string(),
                ContextMenuAction::CopyUrl,
                true,
            ));
        }
        items.push((
            "Edit address".to_string(),
            ContextMenuAction::FocusAddress,
            true,
        ));
        self.context_menu = Some(ContextMenu {
            x,
            y,
            items,
            hover: None,
        });
    }

    /// Execute the chosen context-menu action.
    fn handle_context_menu_action(&mut self, action: ContextMenuAction) {
        match action {
            ContextMenuAction::GoBack => {
                self.active_tab().state.go_back();
            }
            ContextMenuAction::GoForward => {
                self.active_tab().state.go_forward();
            }
            ContextMenuAction::Reload => {
                self.active_tab().state.reload();
            }
            ContextMenuAction::CopyUrl | ContextMenuAction::CopyLink => {
                let url = if action == ContextMenuAction::CopyLink {
                    self.hovered_link_url()
                } else {
                    self.active_tab()
                        .current_url
                        .as_ref()
                        .map(|u| u.to_string())
                };
                if let Some(url) = url {
                    // Best-effort clipboard via the shell provider. Fall back to
                    // logging if the clipboard isn't available.
                    if let Some(doc) = self.active_tab().doc.as_ref() {
                        if doc.shell_provider.set_clipboard_text(url.clone()).is_err() {
                            tracing::info!("clipboard (unavailable) URL: {}", url);
                        }
                    } else {
                        tracing::info!("clipboard (no doc) URL: {}", url);
                    }
                }
            }
            ContextMenuAction::FocusAddress => {
                self.chrome.focus_address();
            }
            ContextMenuAction::Close => {
                self.context_menu = None;
            }
        }
        self.context_menu = None;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = event_loop
            .create_window(
                Window::default_attributes()
                    .with_title("aris")
                    .with_inner_size(winit::dpi::LogicalSize::new(
                        self.config.width,
                        self.config.height,
                    )),
            )
            .expect("create window");
        let window = Rc::new(window);
        self.scale_factor = window.scale_factor();
        let inner = window.inner_size();
        self.phys_size = (inner.width.max(1), inner.height.max(1));
        tracing::info!(
            "resumed: scale_factor={} inner={}x{} (logical {}x{})",
            self.scale_factor,
            inner.width,
            inner.height,
            self.config.width,
            self.config.height
        );

        let context = softbuffer::Context::new(window.clone()).expect("ctx");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("surface");
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);

        self.build_doc();
        if let Some(url) = &self.active_tab().current_url {
            self.active_tab().state.commit_load(url.clone());
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                for tab in &self.tabs {
                    tab.state.save_session();
                }
                event_loop.exit();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
            }
            WindowEvent::Resized(size) => {
                self.scale_factor = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor())
                    .unwrap_or(1.0);
                let pw = (size.width as f64 * self.scale_factor).round() as u32;
                let ph = (size.height as f64 * self.scale_factor).round() as u32;
                self.phys_size = (pw.max(1), ph.max(1));
                self.build_doc();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor;
                self.build_doc();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.process_loads();
                self.pump_messages();
                self.apply_title();
                if self.active_tab().needs_rerender {
                    self.render_base_frame();
                }
                self.present();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let css_x = (position.x / self.scale_factor) as f32;
                let css_y = (position.y / self.scale_factor) as f32;
                self.last_mouse = (css_x, css_y);
                let css_w = self.css_size().0;
                // If dragging the scrollbar, scroll to follow the cursor.
                if self.scrollbar_drag.is_some() {
                    self.scroll_to_drag(css_y);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                // If the context menu is open, track hover over its items.
                if let Some(menu) = self.context_menu.as_mut() {
                    let h = ContextMenu::item_height();
                    menu.hover = (0..menu.items.len()).find(|&i| {
                        let iy = menu.y + 4.0 + i as f32 * h;
                        css_x >= menu.x
                            && css_x <= menu.x + menu.width()
                            && css_y >= iy
                            && css_y < iy + h
                    });
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if css_y < CHROME_HEIGHT_CSS {
                    self.chrome.hover = self.chrome.region_at(css_x, css_y, css_w);
                    self.update_cursor_icon();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                self.chrome.hover = None;
                let page_y = css_y - CHROME_HEIGHT_CSS;
                let hover_changed = self
                    .active_tab_mut()
                    .doc
                    .as_mut()
                    .map(|d| d.set_hover_to(css_x, page_y))
                    .unwrap_or(false);
                self.update_cursor_icon();
                if hover_changed && let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: mstate,
                button,
                ..
            } => {
                if mstate != ElementState::Pressed {
                    // Mouse release ends a scrollbar drag.
                    self.scrollbar_drag = None;
                    return;
                }
                let (cx, cy) = self.last_mouse;
                let css_w = self.css_size().0;

                // Right-click opens a context menu anywhere in the window.
                if button == MouseButton::Right {
                    self.open_context_menu(cx, cy);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if button != MouseButton::Left {
                    return;
                }
                // If a context menu is open, a left click either selects an
                // item or dismisses the menu (click outside).
                if let Some(menu) = self.context_menu.take() {
                    if let Some(action) = menu.item_at(cx, cy) {
                        self.handle_context_menu_action(action);
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if cy < TAB_STRIP_HEIGHT_CSS {
                    // Tab strip: click a tab to switch, click + for new tab.
                    let min_tab = 120.0_f32.min(css_w / (self.tabs.len() as f32 + 1.0).max(1.0));
                    let tab_w = min_tab.max(80.0).min(200.0);
                    let tab_count = self.tabs.len();
                    if cx < tab_w * tab_count as f32 {
                        let idx = (cx / tab_w) as usize;
                        if idx < tab_count && idx != self.active {
                            self.active = idx;
                            let url = self
                                .active_tab()
                                .current_url
                                .as_ref()
                                .map(|u| u.to_string())
                                .unwrap_or_default();
                            self.chrome.set_url(&url);
                            self.chrome.address_focused = false;
                        }
                    } else if cx < tab_w * tab_count as f32 + 40.0 {
                        // "+" new tab button area
                        self.new_tab(None);
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if cy < CHROME_HEIGHT_CSS {
                    if let Some(action) = self.chrome.click_at(cx, cy, css_w) {
                        self.handle_chrome_action(action);
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                self.chrome.address_focused = false;
                // Scrollbar hit-test: the rightmost 10 CSS px of the page area.
                if cx >= css_w - 10.0 && cx < css_w {
                    self.scrollbar_drag = Some(cy);
                    self.scroll_to_drag(cy);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                let page_y = cy - CHROME_HEIGHT_CSS;
                self.dispatch_pointer(cx, page_y, true);
                self.dispatch_pointer(cx, page_y, false);
                self.active_tab_mut().needs_rerender = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (_dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        (x as f64 * 30.0, y as f64 * 30.0)
                    }
                    winit::event::MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                };
                if let Some(doc) = self.active_tab_mut().doc.as_mut() {
                    let hover = doc.get_hover_node_id().unwrap_or(0);
                    doc.scroll_node_by(hover, dy, 0.0, &mut |_| {});
                    self.active_tab_mut().needs_rerender = true;
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                // Address-bar input takes priority when focused.
                if self.chrome.address_focused {
                    if pressed && let Some(action) = self.chrome.handle_key(&event, self.modifiers)
                    {
                        self.handle_chrome_action(action);
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if pressed {
                    // Escape dismisses an open context menu before quitting.
                    if matches!(&event.logical_key, Key::Named(NamedKey::Escape))
                        && self.context_menu.is_some()
                    {
                        self.context_menu = None;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    // Global shortcuts.
                    let ctrl = self.modifiers.control_key() || self.modifiers.super_key();
                    let alt = self.modifiers.alt_key();
                    // Ctrl+F opens the find bar.
                    if ctrl
                        && matches!(&event.logical_key, Key::Character(c) if c.as_str().eq_ignore_ascii_case("f"))
                    {
                        self.open_find();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    // Ctrl+T new tab, Ctrl+W close tab.
                    if ctrl
                        && matches!(&event.logical_key, Key::Character(c) if c.as_str().eq_ignore_ascii_case("t"))
                    {
                        self.new_tab(None);
                        return;
                    }
                    if ctrl
                        && matches!(&event.logical_key, Key::Character(c) if c.as_str().eq_ignore_ascii_case("w"))
                    {
                        self.close_tab();
                        return;
                    }
                    // Ctrl+Tab / Ctrl+Shift+Tab switch tabs.
                    if ctrl && matches!(&event.logical_key, Key::Named(NamedKey::Tab)) {
                        self.switch_tab(!self.modifiers.shift_key());
                        return;
                    }
                    // Ctrl+= / Ctrl+Plus zoom in, Ctrl+- zoom out, Ctrl+0 reset.
                    if ctrl
                        && matches!(
                            &event.logical_key,
                            Key::Character(c) if c.as_str() == "=" || c.as_str() == "+"
                        )
                    {
                        self.active_tab_mut().zoom = (self.active_tab().zoom * 1.2).min(5.0);
                        self.build_doc();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if ctrl && matches!(&event.logical_key, Key::Character(c) if c.as_str() == "-")
                    {
                        self.active_tab_mut().zoom = (self.active_tab().zoom / 1.2).max(0.2);
                        self.build_doc();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if ctrl && matches!(&event.logical_key, Key::Character(c) if c.as_str() == "0")
                    {
                        self.active_tab_mut().zoom = 1.0;
                        self.build_doc();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    // When the find bar is open, it captures keyboard input.
                    if self.active_tab().find.is_some() {
                        match &event.logical_key {
                            Key::Named(NamedKey::Escape) => {
                                self.active_tab_mut().find = None;
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
                                return;
                            }
                            Key::Named(NamedKey::Enter) => {
                                self.find_next(!self.modifiers.shift_key());
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
                                return;
                            }
                            Key::Named(NamedKey::Backspace) => {
                                let q = self
                                    .active_tab()
                                    .find
                                    .as_ref()
                                    .map(|f| {
                                        let mut s = f.query.clone();
                                        s.pop();
                                        s
                                    })
                                    .unwrap_or_default();
                                self.update_find_query(&q);
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
                                return;
                            }
                            Key::Character(c) if !ctrl && !alt && !c.is_empty() => {
                                let q = self
                                    .active_tab()
                                    .find
                                    .as_ref()
                                    .map(|f| format!("{}{}", f.query, c.as_str()))
                                    .unwrap_or_default();
                                self.update_find_query(&q);
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
                                return;
                            }
                            _ => return,
                        }
                    }
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            event_loop.exit();
                            return;
                        }
                        // Alt+Left / Alt+Right: history navigation.
                        Key::Named(NamedKey::ArrowLeft) if alt => {
                            self.active_tab().state.go_back();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        Key::Named(NamedKey::ArrowRight) if alt => {
                            self.active_tab().state.go_forward();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        Key::Named(NamedKey::F5) => {
                            self.active_tab().state.reload();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        Key::Character(c) if ctrl && c.as_str().eq_ignore_ascii_case("r") => {
                            self.active_tab().state.reload();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        Key::Character(c) if ctrl && c.as_str().eq_ignore_ascii_case("l") => {
                            self.chrome.focus_address();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        // Page navigation / scrolling keys (browser-standard).
                        Key::Named(NamedKey::PageDown) => {
                            self.scroll_viewport(false, true);
                            return;
                        }
                        Key::Named(NamedKey::PageUp) => {
                            self.scroll_viewport(true, true);
                            return;
                        }
                        Key::Character(c) if c.as_str() == " " && !self.chrome.address_focused => {
                            // Space scrolls down a page (Shift+Space up).
                            self.scroll_viewport(self.modifiers.shift_key(), true);
                            return;
                        }
                        Key::Named(NamedKey::Home) => {
                            self.scroll_to(0.0);
                            return;
                        }
                        Key::Named(NamedKey::End) => {
                            self.scroll_to(f64::MAX);
                            return;
                        }
                        _ => {}
                    }
                }
                // Forward to the document for text input / page keys.
                if let Some(blitz_evt) = map_winit_key(&event, self.modifiers)
                    && let Some(doc) = self.active_tab_mut().doc.as_mut()
                {
                    let ui = if pressed {
                        UiEvent::KeyDown(blitz_evt)
                    } else {
                        UiEvent::KeyUp(blitz_evt)
                    };
                    doc.handle_ui_event(ui);
                    self.active_tab_mut().needs_rerender = true;
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Close button requested exit — save session first.
        if self.should_quit {
            for tab in &self.tabs {
                tab.state.save_session();
            }
            event_loop.exit();
            return;
        }
        // Drain any favicons fetched by background threads.
        {
            let favicons = std::mem::take(&mut *self.favicon_inbox.lock().unwrap());
            for (idx, fav) in favicons {
                if idx < self.tabs.len() {
                    self.tabs[idx].favicon = Some(fav);
                    if idx == self.active
                        && let Some(w) = &self.window
                    {
                        w.request_redraw();
                    }
                }
            }
        }
        // Poll JS timers (setTimeout/setInterval) for the active tab.
        #[cfg(feature = "js")]
        {
            let tab = self.tabs.get_mut(self.active);
            if let Some(tab) = tab
                && let (Some(rt), Some(doc)) = (tab.js.as_mut(), tab.doc.as_mut())
                && rt.poll_timers(doc)
            {
                tab.needs_rerender = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
        // Drain async loads / redraw requests from providers.
        if self
            .active_tab()
            .state
            .redraw_requested
            .swap(false, Ordering::Relaxed)
        {
            self.process_loads();
            self.pump_messages();
            self.apply_title();
            if self.active_tab().needs_rerender {
                self.render_base_frame();
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Blink the caret.
        let now = Instant::now();
        if now.duration_since(self.last_caret) >= Duration::from_millis(530) {
            self.last_caret = now;
            self.chrome.toggle_caret();
            if self.chrome.address_focused
                && let Some(window) = &self.window
            {
                window.request_redraw();
            }
        }
    }
}

// ── Chrome (address bar + nav buttons) ──────────────────────

/// Layout rectangles for the chrome bar, in CSS logical px.
struct ChromeLayout {
    back: (f32, f32, f32, f32),
    forward: (f32, f32, f32, f32),
    reload: (f32, f32, f32, f32),
    favicon: (f32, f32, f32, f32),
    address: (f32, f32, f32, f32),
    close: (f32, f32, f32, f32),
}

impl ChromeLayout {
    fn compute(width: f32) -> Self {
        // Layout is relative to the chrome bar, which sits below the tab strip.
        let y0 = TAB_STRIP_HEIGHT_CSS; // top of chrome bar
        let h = CHROME_BAR_HEIGHT_CSS; // chrome bar height
        let pad = 8.0;
        let btn = 28.0;
        let fav = 18.0;
        let mut x = pad;
        let back = (x, y0 + (h - btn) / 2.0, btn, btn);
        x += btn + 4.0;
        let forward = (x, y0 + (h - btn) / 2.0, btn, btn);
        x += btn + 4.0;
        let reload = (x, y0 + (h - btn) / 2.0, btn, btn);
        x += btn + 8.0;
        let favicon = (x, y0 + (h - fav) / 2.0, fav, fav);
        x += fav + 6.0;
        let close = (width - pad - btn, y0 + (h - btn) / 2.0, btn, btn);
        let addr_w = (close.0 - x - 6.0).max(60.0);
        let address = (x, y0 + (h - 26.0) / 2.0, addr_w, 26.0);
        Self {
            back,
            forward,
            reload,
            favicon,
            address,
            close,
        }
    }
}

enum ChromeAction {
    GoBack,
    GoForward,
    Reload,
    Navigate(String),
    CloseWindow,
    RedrawOnly,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ChromeRegion {
    Back,
    Forward,
    Reload,
    Address,
    Close,
}

/// A right-click context menu overlay. `None` means no menu is open.
#[derive(Clone)]
pub struct ContextMenu {
    /// Top-left of the menu, in CSS px relative to the window.
    pub x: f32,
    pub y: f32,
    /// Items: (label, action, enabled).
    pub items: Vec<(String, ContextMenuAction, bool)>,
    /// Index of the currently hovered item, if any.
    pub hover: Option<usize>,
}

/// What a context-menu item does when clicked.
#[derive(Clone, Copy, PartialEq)]
pub enum ContextMenuAction {
    GoBack,
    GoForward,
    Reload,
    CopyUrl,
    CopyLink,
    FocusAddress,
    Close,
}

impl ContextMenu {
    fn item_height() -> f32 {
        26.0
    }
    fn min_width() -> f32 {
        160.0
    }

    fn height(&self) -> f32 {
        Self::item_height() * self.items.len() as f32 + 8.0
    }
    fn width(&self) -> f32 {
        Self::min_width()
    }

    /// Hit-test a CSS-px point. Returns the action if a menu item was clicked.
    fn item_at(&self, x: f32, y: f32) -> Option<ContextMenuAction> {
        let h = Self::item_height();
        if x < self.x || x > self.x + self.width() {
            return None;
        }
        for (i, (_, action, enabled)) in self.items.iter().enumerate() {
            let iy = self.y + 4.0 + i as f32 * h;
            if y >= iy && y < iy + h && *enabled {
                return Some(*action);
            }
        }
        None
    }
}

struct ChromeState {
    address: String,
    address_focused: bool,
    hover: Option<ChromeRegion>,
    caret_blink: bool,
}

impl ChromeState {
    fn new() -> Self {
        Self {
            address: String::new(),
            address_focused: false,
            hover: None,
            caret_blink: true,
        }
    }

    fn set_url(&mut self, url: &str) {
        self.address = url.to_string();
    }

    fn toggle_caret(&mut self) {
        self.caret_blink = !self.caret_blink;
    }

    fn caret_visible(&self) -> bool {
        self.address_focused && self.caret_blink
    }

    fn focus_address(&mut self) {
        self.address_focused = true;
        self.caret_blink = true;
    }

    fn region_at(&self, x: f32, y: f32, width: f32) -> Option<ChromeRegion> {
        let l = ChromeLayout::compute(width);
        let in_rect =
            |r: (f32, f32, f32, f32)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
        if in_rect(l.back) {
            Some(ChromeRegion::Back)
        } else if in_rect(l.forward) {
            Some(ChromeRegion::Forward)
        } else if in_rect(l.reload) {
            Some(ChromeRegion::Reload)
        } else if in_rect(l.close) {
            Some(ChromeRegion::Close)
        } else if in_rect(l.address) {
            Some(ChromeRegion::Address)
        } else {
            None
        }
    }

    fn click_at(&mut self, x: f32, y: f32, width: f32) -> Option<ChromeAction> {
        match self.region_at(x, y, width)? {
            ChromeRegion::Back => Some(ChromeAction::GoBack),
            ChromeRegion::Forward => Some(ChromeAction::GoForward),
            ChromeRegion::Reload => Some(ChromeAction::Reload),
            ChromeRegion::Close => Some(ChromeAction::CloseWindow),
            ChromeRegion::Address => {
                self.address_focused = true;
                self.caret_blink = true;
                Some(ChromeAction::RedrawOnly)
            }
        }
    }

    fn handle_key(
        &mut self,
        event: &winit::event::KeyEvent,
        modifiers: ModifiersState,
    ) -> Option<ChromeAction> {
        if event.state != ElementState::Pressed {
            return None;
        }
        let ctrl = modifiers.control_key() || modifiers.super_key();
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                self.address_focused = false;
                Some(ChromeAction::Navigate(self.address.clone()))
            }
            Key::Named(NamedKey::Escape) => {
                self.address_focused = false;
                Some(ChromeAction::RedrawOnly)
            }
            Key::Named(NamedKey::Backspace) => {
                self.address.pop();
                self.caret_blink = true;
                Some(ChromeAction::RedrawOnly)
            }
            Key::Character(c) if !ctrl => {
                let s = c.as_str();
                if !s.is_empty() && s.chars().all(|ch| !ch.is_control()) {
                    self.address.push_str(s);
                    self.caret_blink = true;
                    Some(ChromeAction::RedrawOnly)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

// ── Key mapping ─────────────────────────────────────────────

/// Map a winit key event to a blitz `BlitzKeyEvent`.
fn map_winit_key(
    event: &winit::event::KeyEvent,
    modifiers: ModifiersState,
) -> Option<blitz_traits::events::BlitzKeyEvent> {
    let key = winit_key_to_kbd(&event.logical_key);
    let code = winit_physical_to_code(&event.physical_key);
    let state = if event.state == ElementState::Pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    };
    let text = if let winit::keyboard::Key::Character(c) = &event.logical_key {
        Some(c.to_string())
    } else {
        None
    };
    Some(blitz_traits::events::BlitzKeyEvent {
        key,
        code,
        modifiers: winit_modifiers_to_blitz(modifiers),
        location: keyboard_types::Location::Standard,
        is_auto_repeating: event.repeat,
        is_composing: false,
        state,
        text: text.map(Into::into),
    })
}

fn winit_key_to_kbd(key: &winit::keyboard::Key) -> keyboard_types::Key {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Named(n) => match n {
            NamedKey::Enter => keyboard_types::Key::Enter,
            NamedKey::Backspace => keyboard_types::Key::Backspace,
            NamedKey::Tab => keyboard_types::Key::Tab,
            NamedKey::Escape => keyboard_types::Key::Escape,
            // Space has no dedicated variant in keyboard-types 0.7; it is
            // surfaced as Character(" ").
            NamedKey::Space => keyboard_types::Key::Character(" ".to_string()),
            NamedKey::ArrowLeft => keyboard_types::Key::ArrowLeft,
            NamedKey::ArrowRight => keyboard_types::Key::ArrowRight,
            NamedKey::ArrowUp => keyboard_types::Key::ArrowUp,
            NamedKey::ArrowDown => keyboard_types::Key::ArrowDown,
            NamedKey::Home => keyboard_types::Key::Home,
            NamedKey::End => keyboard_types::Key::End,
            NamedKey::Delete => keyboard_types::Key::Delete,
            NamedKey::PageUp => keyboard_types::Key::PageUp,
            NamedKey::PageDown => keyboard_types::Key::PageDown,
            NamedKey::Shift => keyboard_types::Key::Shift,
            NamedKey::Control => keyboard_types::Key::Control,
            NamedKey::Alt => keyboard_types::Key::Alt,
            NamedKey::Super => keyboard_types::Key::Meta,
            NamedKey::CapsLock => keyboard_types::Key::CapsLock,
            _ => keyboard_types::Key::Unidentified,
        },
        Key::Character(c) => keyboard_types::Key::Character(c.to_string()),
        _ => keyboard_types::Key::Unidentified,
    }
}

fn winit_physical_to_code(key: &winit::keyboard::PhysicalKey) -> keyboard_types::Code {
    use winit::keyboard::PhysicalKey;
    match key {
        PhysicalKey::Unidentified(_) => keyboard_types::Code::Unidentified,
        PhysicalKey::Code(c) => {
            let name = format!("{:?}", c);
            name.parse().unwrap_or(keyboard_types::Code::Unidentified)
        }
    }
}

fn winit_modifiers_to_blitz(m: ModifiersState) -> keyboard_types::Modifiers {
    let mut out = keyboard_types::Modifiers::empty();
    if m.control_key() {
        out |= keyboard_types::Modifiers::CONTROL;
    }
    if m.alt_key() {
        out |= keyboard_types::Modifiers::ALT;
    }
    if m.shift_key() {
        out |= keyboard_types::Modifiers::SHIFT;
    }
    if m.super_key() {
        out |= keyboard_types::Modifiers::META;
    }
    out
}

// ── Chrome drawing ──────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
/// Draw the tab strip at the top of the window: one rectangle per tab with
/// its title, the active tab highlighted.
fn draw_tab_strip(
    buffer: &mut [u32],
    buf_w: usize,
    scale: f32,
    css_w: f32,
    titles: &[String],
    active: usize,
) {
    let strip_h = (TAB_STRIP_HEIGHT_CSS * scale) as usize;
    let bg = 0x16161E;
    let tab_bg = 0x2D2D3A;
    let tab_active_bg = 0x3D3D50;
    let tab_text = 0xC0CAF5;
    let tab_text_dim = 0x6B6B80;
    let new_btn_bg = 0x2D2D3A;

    // Fill strip background.
    for y in 0..strip_h {
        let row = y * buf_w;
        for x in 0..buf_w {
            if row + x < buffer.len() {
                buffer[row + x] = bg;
            }
        }
    }

    // Each tab is min 120px, max 200px wide (CSS). Plus a "+" new-tab button.
    let min_tab = 120.0_f32.min(css_w / (titles.len() as f32 + 1.0).max(1.0));
    let max_tab = 200.0;
    let tab_w = min_tab.max(80.0).min(max_tab);
    let close_btn = 14.0; // close X area
    let text_pad = 10.0;

    let mut x = 4.0;
    for (i, title) in titles.iter().enumerate() {
        let tw = tab_w;
        let tx = (x * scale) as usize;
        let ty = 2;
        let th = strip_h - 2;
        let tw_phys = (tw * scale) as usize;
        let color = if i == active { tab_active_bg } else { tab_bg };
        for ry in 0..th {
            for rx in 0..tw_phys {
                let idx = (ty + ry) * buf_w + (tx + rx);
                if idx < buffer.len() {
                    buffer[idx] = color;
                }
            }
        }
        // Tab title text (truncated to fit). Close X is drawn as a simple mark.
        let display_title = if title.len() > 16 {
            format!("{}…", &title[..14])
        } else {
            title.clone()
        };
        let avail_text = (tw - close_btn - text_pad).max(40.0) as usize;
        if let Some(strip) = render_text_strip_colored(
            &display_title,
            (avail_text as f32 * scale) as usize,
            th,
            scale,
            if i == active { "#c0caf5" } else { "#9aa5ce" },
        ) {
            blit_strip(
                buffer,
                buf_w,
                strip_h,
                &strip,
                tx + (text_pad * scale) as usize,
                ty + 2,
            );
        }
        // Close × (small, top-right of tab).
        {
            let cx = tx + tw_phys - (close_btn * scale) as usize;
            let cy = th / 2;
            let half = 3;
            for d in -half..=half {
                plot(
                    buffer,
                    buf_w,
                    cx + d as usize,
                    cy + d as usize,
                    tab_text_dim,
                );
                plot(
                    buffer,
                    buf_w,
                    cx + d as usize,
                    cy - d as usize + 2 * cy,
                    tab_text_dim,
                );
            }
        }
        x += tw;
    }

    // "+" new-tab button.
    let plus_x = (x * scale) as usize + (8.0 * scale) as usize;
    let plus_cy = strip_h / 2;
    let plus_size = 6.0_f32 * scale;
    // Background circle.
    for ry in 0..(plus_size * 2.0) as usize {
        for rx in 0..(plus_size * 2.0) as usize {
            let dx = rx as f32 - plus_size;
            let dy = ry as f32 - plus_size;
            if dx.hypot(dy) <= plus_size {
                plot(
                    buffer,
                    buf_w,
                    plus_x + rx,
                    plus_cy - plus_size as usize + ry,
                    new_btn_bg,
                );
            }
        }
    }
    // + cross.
    for d in 0..(plus_size as usize) {
        plot(
            buffer,
            buf_w,
            plus_x + plus_size as usize,
            plus_cy - d,
            tab_text,
        );
        plot(
            buffer,
            buf_w,
            plus_x + plus_size as usize,
            plus_cy + d,
            tab_text,
        );
        plot(
            buffer,
            buf_w,
            plus_x + plus_size as usize - d,
            plus_cy,
            tab_text,
        );
        plot(
            buffer,
            buf_w,
            plus_x + plus_size as usize + d,
            plus_cy,
            tab_text,
        );
    }
}

pub fn draw_chrome(
    buffer: &mut [u32],
    buf_w: usize,
    chrome_h: usize,
    css_w: f32,
    scale: f32,
    display: &str,
    focused: bool,
    caret_on: bool,
    can_back: bool,
    can_fwd: bool,
    hover: Option<ChromeRegion>,
) {
    let bg = 0x2D2D3A;
    let btn_color = 0xC8C8D8;
    let btn_hover = 0xFFFFFF;
    let btn_disabled = 0x555566;
    let addr_bg = if focused { 0x1A1B26 } else { 0x24243A };
    let addr_border = if focused { 0x7AA2F7 } else { 0x3A3A4E };
    let accent = 0x7AA2F7;

    // Fill chrome background.
    for y in 0..chrome_h {
        let row = y * buf_w;
        for x in 0..buf_w {
            if row + x < buffer.len() {
                buffer[row + x] = bg;
            }
        }
    }

    let layout = ChromeLayout::compute(css_w);
    let to_phys = |v: f32| (v * scale) as usize;

    // Navigation buttons: render Lucide-style stroke icons (arrow-left /
    // arrow-right / rotate-ccw) defined in a 24x24 viewBox, scaled into each
    // button rect. Lucide path data:
    //   arrow-left : M19 12H5  M12 19l-7-7 7-7
    //   arrow-right: M5 12h14  M12 19l7-7-7-7
    //   rotate-ccw : M3 2v6h6  M3 13a9 9 0 1 0 3-7.7L3 8
    let draw_back = |buf: &mut [u32], rect: (f32, f32, f32, f32), enabled: bool, hovered: bool| {
        let color = if !enabled {
            btn_disabled
        } else if hovered {
            btn_hover
        } else {
            btn_color
        };
        let (ox, oy, unit, _w, _h) = icon_origin(rect, scale);
        let stroke = 1.8 * unit;
        // horizontal line 19,12 -> 5,12
        draw_line(
            buf,
            buf_w,
            ox + 19.0 * unit,
            oy + 12.0 * unit,
            ox + 5.0 * unit,
            oy + 12.0 * unit,
            stroke,
            color,
        );
        // arrowhead 12,19 -> 5,12 -> 12,5
        draw_line(
            buf,
            buf_w,
            ox + 12.0 * unit,
            oy + 19.0 * unit,
            ox + 5.0 * unit,
            oy + 12.0 * unit,
            stroke,
            color,
        );
        draw_line(
            buf,
            buf_w,
            ox + 12.0 * unit,
            oy + 5.0 * unit,
            ox + 5.0 * unit,
            oy + 12.0 * unit,
            stroke,
            color,
        );
    };
    let draw_forward =
        |buf: &mut [u32], rect: (f32, f32, f32, f32), enabled: bool, hovered: bool| {
            let color = if !enabled {
                btn_disabled
            } else if hovered {
                btn_hover
            } else {
                btn_color
            };
            let (ox, oy, unit, _w, _h) = icon_origin(rect, scale);
            let stroke = 1.8 * unit;
            // horizontal line 5,12 -> 19,12
            draw_line(
                buf,
                buf_w,
                ox + 5.0 * unit,
                oy + 12.0 * unit,
                ox + 19.0 * unit,
                oy + 12.0 * unit,
                stroke,
                color,
            );
            // arrowhead 12,19 -> 19,12 -> 12,5
            draw_line(
                buf,
                buf_w,
                ox + 12.0 * unit,
                oy + 19.0 * unit,
                ox + 19.0 * unit,
                oy + 12.0 * unit,
                stroke,
                color,
            );
            draw_line(
                buf,
                buf_w,
                ox + 12.0 * unit,
                oy + 5.0 * unit,
                ox + 19.0 * unit,
                oy + 12.0 * unit,
                stroke,
                color,
            );
        };
    draw_back(
        buffer,
        layout.back,
        can_back,
        hover == Some(ChromeRegion::Back),
    );
    draw_forward(
        buffer,
        layout.forward,
        can_fwd,
        hover == Some(ChromeRegion::Forward),
    );

    // Reload: rotate-ccw. L-shaped corner + a near-full circle (arc).
    {
        let rect = layout.reload;
        let color = if hover == Some(ChromeRegion::Reload) {
            btn_hover
        } else {
            btn_color
        };
        let (ox, oy, unit, _w, _h) = icon_origin(rect, scale);
        let stroke = 1.8 * unit;
        // corner: M3 2v6h6  -> (3,2)->(3,8)->(9,8)
        draw_line(
            buffer,
            buf_w,
            ox + 3.0 * unit,
            oy + 2.0 * unit,
            ox + 3.0 * unit,
            oy + 8.0 * unit,
            stroke,
            color,
        );
        draw_line(
            buffer,
            buf_w,
            ox + 3.0 * unit,
            oy + 8.0 * unit,
            ox + 9.0 * unit,
            oy + 8.0 * unit,
            stroke,
            color,
        );
        // arc: M3 13a9 9 0 1 0 3-7.7 -> circle centered ~(12,13) r~9. The point
        // (3,13) sits at 180° from center. Sweep clockwise ~330° (leave a gap
        // at the top where the arrowhead corner sits).
        draw_circle(
            buffer,
            buf_w,
            ox + 12.0 * unit,
            oy + 13.0 * unit,
            9.0 * unit,
            stroke,
            color,
            180.0_f32.to_radians(),
            330.0_f32.to_radians(),
        );
    }

    // Favicon slot: colored rounded square as fallback. Real favicons are
    // drawn as an overlay in present() after draw_chrome returns.
    {
        let rect = layout.favicon;
        let color = favicon_color(display);
        let fx = to_phys(rect.0) as i32;
        let fy = to_phys(rect.1) as i32;
        let fw = to_phys(rect.2) as i32;
        let fh = to_phys(rect.3) as i32;
        let radius = (fw as f32 * 0.22) as i32;
        fill_rounded_rect(buffer, buf_w, fx, fy, fw, fh, radius, color);
    }

    // Close button: Lucide X (M18 6 6 18 / M6 6l12 12), reddish on hover.
    {
        let rect = layout.close;
        let color = if hover == Some(ChromeRegion::Close) {
            0xF7768E // red on hover
        } else {
            btn_color
        };
        let (ox, oy, unit, _w, _h) = icon_origin(rect, scale);
        let stroke = 1.8 * unit;
        draw_line(
            buffer,
            buf_w,
            ox + 18.0 * unit,
            oy + 6.0 * unit,
            ox + 6.0 * unit,
            oy + 18.0 * unit,
            stroke,
            color,
        );
        draw_line(
            buffer,
            buf_w,
            ox + 6.0 * unit,
            oy + 6.0 * unit,
            ox + 18.0 * unit,
            oy + 18.0 * unit,
            stroke,
            color,
        );
    }

    // Address bar background + border.
    let ax = to_phys(layout.address.0);
    let ay = to_phys(layout.address.1);
    let aw = to_phys(layout.address.2);
    let ah = to_phys(layout.address.3);
    for y in 0..ah {
        for x in 0..aw {
            let bx = ax + x;
            let by = ay + y;
            if by < chrome_h {
                let idx = by * buf_w + bx;
                if idx < buffer.len() {
                    let on_border = x == 0 || x == aw - 1 || y == 0 || y == ah - 1;
                    buffer[idx] = if on_border { addr_border } else { addr_bg };
                }
            }
        }
    }

    // Render the address-bar text via the same Vello pipeline used for page
    // content, then alpha-blend it onto the chrome bar. This gives crisp,
    // real (not indicator-bar) text at the display's full resolution.
    let text_pad = to_phys(6.0);
    let avail_w = aw.saturating_sub(text_pad * 2);
    if !display.is_empty() && avail_w > 4 {
        // Render at physical resolution; the strip is (avail_w x ah).
        if let Some(glyphs) = render_text_strip(display, avail_w, ah, scale) {
            let gw = glyphs.width;
            let gh = glyphs.height;
            // Visible text width (clip to available).
            let vis_w = gw.min(avail_w);
            for ty in 0..gh.min(ah) {
                for tx in 0..vis_w {
                    let sidx = (ty * gw + tx) * 4;
                    if sidx + 3 >= glyphs.rgba.len() {
                        continue;
                    }
                    let a = glyphs.rgba[sidx + 3] as u32;
                    if a == 0 {
                        continue;
                    }
                    let bx = ax + text_pad + tx;
                    let by = ay + ty;
                    if by < chrome_h && bx < buf_w {
                        let idx = by * buf_w + bx;
                        if idx < buffer.len() {
                            let tr = glyphs.rgba[sidx] as u32;
                            let tg = glyphs.rgba[sidx + 1] as u32;
                            let tb = glyphs.rgba[sidx + 2] as u32;
                            let dst = buffer[idx];
                            let dr = (dst >> 16) & 0xFF;
                            let dg = (dst >> 8) & 0xFF;
                            let db = dst & 0xFF;
                            let nr = (tr * a + dr * (255 - a)) / 255;
                            let ng = (tg * a + dg * (255 - a)) / 255;
                            let nb = (tb * a + db * (255 - a)) / 255;
                            buffer[idx] = (nr << 16) | (ng << 8) | nb;
                        }
                    }
                }
            }
            // Caret just past the rendered text.
            if focused && caret_on {
                let cx = ax + text_pad + vis_w + 1;
                for y in 3..ah.saturating_sub(3) {
                    let by = ay + y;
                    if by < chrome_h && cx < buf_w {
                        let idx = by * buf_w + cx;
                        if idx < buffer.len() {
                            buffer[idx] = accent;
                        }
                    }
                }
            }
        }
    }
}

fn plot(buffer: &mut [u32], buf_w: usize, x: usize, y: usize, color: u32) {
    if (0..buf_w).contains(&x) {
        let idx = y * buf_w + x;
        if idx < buffer.len() {
            buffer[idx] = color;
        }
    }
}

/// Derive a stable, pleasant color (XRGB) for a URL by hashing its host. Empty
/// or local input yields a neutral gray, so the slot reads as "no favicon".
/// Detect whether the OS is in dark mode, so pages using
/// `prefers-color-scheme: dark` render correctly.
#[cfg(target_os = "windows")]
fn is_os_dark_mode() -> bool {
    use std::process::Command;
    // Check the registry: HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme
    // 0 = dark, 1 = light.
    let out = Command::new("reg")
        .args([
            "query",
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
            "/v",
            "AppsUseLightTheme",
        ])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            // If AppsUseLightTheme is 0x0, dark mode is on.
            !s.contains("0x1")
        }
        Err(_) => false,
    }
}

#[cfg(not(target_os = "windows"))]
fn is_os_dark_mode() -> bool {
    false
}

fn favicon_color(url: &str) -> u32 {
    if url.trim().is_empty() {
        return 0x4A4A5E;
    }
    // Extract the host portion if this is a URL.
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url);
    let mut h: u32 = 2166136261;
    for b in host.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    // Map the hash into HSL space with fixed S/L for a soft, distinct color.
    let hue = (h % 360) as f32;
    let (r, g, b) = hsl_to_rgb(hue, 0.55, 0.55);
    (r << 16) | (g << 8) | b
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u32, u32, u32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0).round() as u32,
        ((g1 + m) * 255.0).round() as u32,
        ((b1 + m) * 255.0).round() as u32,
    )
}

/// Scrollbar geometry, in CSS px.
struct ScrollbarMetrics {
    content_h: f32,
    viewport_h: f32,
    scroll_y: f32,
    _chrome_phys: f32,
}

/// Draw a vertical scrollbar on the right edge of the page area.
fn draw_scrollbar(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    chrome_phys: usize,
    scale: f32,
    sb: ScrollbarMetrics,
) {
    let track_w = (10.0 * scale) as usize;
    let track_x = buf_w.saturating_sub(track_w);
    let track_y = chrome_phys;
    let track_h = buf_h.saturating_sub(chrome_phys);
    if track_h == 0 || sb.content_h <= 0.0 {
        return;
    }
    // Track background (subtle).
    let track_bg = 0x111118;
    for y in 0..track_h {
        for x in 0..track_w {
            let idx = (track_y + y) * buf_w + (track_x + x);
            if idx < buffer.len() {
                buffer[idx] = track_bg;
            }
        }
    }
    // Thumb: height proportional to viewport/content, position by scroll_y.
    let thumb_h = ((sb.viewport_h / sb.content_h) * track_h as f32) as usize;
    let thumb_h = thumb_h.max(track_w); // never smaller than the track width
    let max_scroll = (sb.content_h - sb.viewport_h).max(1.0);
    let thumb_y = track_y + ((sb.scroll_y / max_scroll) * (track_h - thumb_h) as f32) as usize;
    let thumb_color = 0x6E6E80;
    let thumb_h = thumb_h.min(track_h);
    for y in 0..thumb_h {
        for x in 0..track_w {
            let idx = (thumb_y + y) * buf_w + (track_x + x);
            if idx < buffer.len() {
                buffer[idx] = thumb_color;
            }
        }
    }
}

/// Draw a slim bottom status bar showing the link URL or a loading message.
fn draw_status_bar(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    scale: f32,
    _css_w: f32,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let bar_h = (18.0 * scale) as usize;
    let by = buf_h.saturating_sub(bar_h);
    let bg = 0x1F2030;
    for y in 0..bar_h {
        for x in 0..buf_w {
            let idx = (by + y) * buf_w + x;
            if idx < buffer.len() {
                buffer[idx] = bg;
            }
        }
    }
    // Render the status text.
    let label_w = buf_w.saturating_sub(16);
    let label_h = (bar_h as f32 * 0.7) as usize;
    if let Some(strip) = render_text_strip_colored(text, label_w, label_h, scale, "#9aa5ce") {
        let tx = 8;
        let ty = by + (bar_h.saturating_sub(strip.height)) / 2;
        blit_strip(buffer, buf_w, buf_h, &strip, tx, ty);
    }
}

/// Draw find-in-page match highlights + the find bar (top-right).
fn draw_find_overlays(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    chrome_phys: usize,
    scale: f32,
    css_w: f32,
    find: &FindState,
) {
    // Match highlights (page CSS px → physical, offset by chrome).
    let normal = 0xF9E2AF; // soft yellow
    let active = 0xFF9E64; // orange for the current match
    for (i, (x, y, w, h)) in find.matches.iter().enumerate() {
        let color = if i == find.active { active } else { normal };
        let px = (*x * scale) as i32;
        let py = (*y * scale) as i32 + chrome_phys as i32;
        let pw = (*w * scale) as i32;
        let ph = (*h * scale) as i32;
        for ry in 0..ph {
            for rx in 0..pw {
                let bx = px + rx;
                let by = py + ry;
                if bx >= 0 && (bx as usize) < buf_w && by >= 0 && (by as usize) < buf_h {
                    let idx = by as usize * buf_w + bx as usize;
                    if idx < buffer.len() {
                        // Alpha-blend ~40% over the existing pixel.
                        let dst = buffer[idx];
                        let dr = (dst >> 16) & 0xFF;
                        let dg = (dst >> 8) & 0xFF;
                        let db = dst & 0xFF;
                        let sr = ((color >> 16) & 0xFF) as u32;
                        let sg = ((color >> 8) & 0xFF) as u32;
                        let sb = (color & 0xFF) as u32;
                        let nr = (sr * 120 + dr * 135) / 255;
                        let ng = (sg * 120 + dg * 135) / 255;
                        let nb = (sb * 120 + db * 135) / 255;
                        buffer[idx] = (nr << 16) | (ng << 8) | nb;
                    }
                }
            }
        }
    }

    // Find bar: a small panel top-right below the chrome, showing the query
    // (text rendered via the strip pipeline) and the match count.
    let bar_w_css = 220.0_f32.min(css_w - 16.0);
    let bar_h_css = 32.0;
    let bar_x_css = css_w - bar_w_css - 8.0;
    let bar_y_css = CHROME_HEIGHT_CSS + 4.0;
    let bx = (bar_x_css * scale) as usize;
    let by = (bar_y_css * scale) as usize;
    let bw = (bar_w_css * scale) as usize;
    let bh = (bar_h_css * scale) as usize;
    let bg = 0x24243A;
    let border = 0x3A3A4E;
    for ry in 0..bh {
        for rx in 0..bw {
            let _idx = by * buf_w + bx + rx + ry * 0;
            let idx = (by + ry) * buf_w + (bx + rx);
            if idx < buffer.len() {
                let on_border = rx == 0 || rx == bw - 1 || ry == 0 || ry == bh - 1;
                buffer[idx] = if on_border { border } else { bg };
            }
        }
    }
    // Render the query text + match count.
    let label = if find.query.is_empty() {
        "Find:".to_string()
    } else {
        format!(
            "Find: {} ({}/{})",
            find.query,
            find.active + 1,
            find.matches.len()
        )
    };
    let label_w = bw.saturating_sub(16);
    let label_h = (bar_h_css * 0.6) as usize;
    if let Some(strip) = render_text_strip_colored(&label, label_w, label_h, scale, "#9aa5ce") {
        let tx = bx + 8;
        let ty = by + (bh.saturating_sub(strip.height)) / 2;
        blit_strip(buffer, buf_w, buf_h, &strip, tx, ty);
    }
}

/// Fill an axis-aligned rounded rectangle.
fn fill_rounded_rect(
    buffer: &mut [u32],
    buf_w: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: u32,
) {
    let r = radius.min(w / 2).min(h / 2);
    for ry in 0..h {
        for rx in 0..w {
            // Corner test: distance from the nearest corner center.
            let mut inside = true;
            let cx = rx;
            let cy = ry;
            for (ccx, ccy) in [
                (r, r),
                (w - r - 1, r),
                (r, h - r - 1),
                (w - r - 1, h - r - 1),
            ] {
                if (cx < r || cx > w - r - 1) && (cy < r || cy > h - r - 1) {
                    let dx = (cx - ccx) as f32;
                    let dy = (cy - ccy) as f32;
                    if dx.hypot(dy) > r as f32 {
                        inside = false;
                        break;
                    }
                }
            }
            if inside {
                plot(buffer, buf_w, (x + rx) as usize, (y + ry) as usize, color);
            }
        }
    }
}

/// Compute the top-left origin (in physical px) of a 24×24 icon inside a
/// button rect, centered. Returns `(ox, oy, unit, icon_phys_w, icon_phys_h)`
/// where `unit` is the physical-pixels-per-viewBox-unit scale. Icon path
/// coordinates are in the 0..24 viewBox; multiply by `unit` and add `ox`/`oy`.
fn icon_origin(rect: (f32, f32, f32, f32), scale: f32) -> (f32, f32, f32, f32, f32) {
    let bw_phys = rect.2 * scale;
    let bh_phys = rect.3 * scale;
    let icon_phys = bw_phys.min(bh_phys) * 0.72;
    let unit = icon_phys / 24.0;
    let ox = rect.0 * scale + (bw_phys - icon_phys) / 2.0;
    let oy = rect.1 * scale + (bh_phys - icon_phys) / 2.0;
    (ox, oy, unit, icon_phys, icon_phys)
}

/// Draw a thick line between two points (physical px), rasterized into
/// `buffer` by stamping small discs along the segment.
fn draw_line(
    buffer: &mut [u32],
    buf_w: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    width: f32,
    color: u32,
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = dx.hypot(dy);
    if len < 0.5 {
        return;
    }
    let steps = (len * 2.0).ceil() as i32;
    let half = width / 2.0;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let cx = x0 + dx * t;
        let cy = y0 + dy * t;
        let r = half.ceil() as i32;
        for py in -r..=r {
            for px in -r..=r {
                if (px as f32).hypot(py as f32) <= half {
                    plot(
                        buffer,
                        buf_w,
                        (cx + px as f32) as usize,
                        (cy + py as f32) as usize,
                        color,
                    );
                }
            }
        }
    }
}

/// Draw a stroked circle (or arc) centered at (cx, cy) in physical px.
///
/// `start_rad` is the angle (radians, 0 = +x axis, increasing clockwise in
/// screen coords) where the arc begins; `sweep_rad` is how far it sweeps
/// (positive = clockwise/increasing angle). Pass `TAU` for a full circle.
fn draw_circle(
    buffer: &mut [u32],
    buf_w: usize,
    cx: f32,
    cy: f32,
    radius: f32,
    width: f32,
    color: u32,
    start_rad: f32,
    sweep_rad: f32,
) {
    let arc_len = radius * sweep_rad.abs();
    let steps = (arc_len * 2.0).ceil() as i32;
    let half = width / 2.0;
    let r = half.ceil() as i32;
    for i in 0..=steps {
        let frac = if steps > 0 {
            i as f32 / steps as f32
        } else {
            0.0
        };
        let a = start_rad + frac * sweep_rad;
        let x = cx + a.cos() * radius;
        let y = cy + a.sin() * radius;
        for py in -r..=r {
            for px in -r..=r {
                if (px as f32).hypot(py as f32) <= half {
                    plot(
                        buffer,
                        buf_w,
                        x as usize + px as usize,
                        y as usize + py as usize,
                        color,
                    );
                }
            }
        }
    }
}

/// Draw the right-click context menu overlay, with text labels.
pub fn draw_context_menu(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    scale: f32,
    menu: &ContextMenu,
) {
    let mw = menu.width() * scale;
    let mh = menu.height() * scale;
    let mx = (menu.x * scale) as i32;
    let my = (menu.y * scale) as i32;
    let item_h = (ContextMenu::item_height() * scale) as i32;

    let bg = 0x2D2D3A;
    let border = 0x3A3A4E;
    let hover_bg = 0x4A6FCE;

    // Background + border.
    for ry in 0..(mh as i32) {
        for rx in 0..(mw as i32) {
            let x = mx + rx;
            let y = my + ry;
            if x >= 0 && (x as usize) < buf_w && y >= 0 && (y as usize) < buf_h {
                let idx = y as usize * buf_w + x as usize;
                if idx < buffer.len() {
                    let on_border =
                        rx == 0 || rx == mw as i32 - 1 || ry == 0 || ry == mh as i32 - 1;
                    buffer[idx] = if on_border { border } else { bg };
                }
            }
        }
    }
    // Hover highlight rect on the selected item.
    if let Some(hi) = menu.hover {
        let iy = my + (4.0 * scale) as i32 + hi as i32 * item_h;
        for ry in 0..item_h {
            for rx in 2..(mw as i32 - 2) {
                let x = mx + rx;
                let y = iy + ry;
                if x >= 0 && (x as usize) < buf_w && y >= 0 && (y as usize) < buf_h {
                    let idx = y as usize * buf_w + x as usize;
                    if idx < buffer.len() {
                        buffer[idx] = hover_bg;
                    }
                }
            }
        }
    }

    // Text labels for each item, rendered via the same Vello pipeline as the
    // address bar so they're crisp at any DPI.
    let text_pad = (10.0 * scale) as usize;
    let label_w = mw as usize - text_pad * 2;
    let item_h_us = item_h as usize;
    let css_h = (ContextMenu::item_height() * 0.6) as usize;
    for (i, (label, _action, enabled)) in menu.items.iter().enumerate() {
        let color = if *enabled { "#d5d5e8" } else { "#5a5a6e" };
        let strip = render_text_strip_colored(label, label_w, css_h, scale, color);
        if let Some(strip) = strip {
            // Vertically center within the item row.
            let ty = (my as usize)
                + (4.0 * scale) as usize
                + i * item_h_us
                + (item_h_us.saturating_sub(strip.height)) / 2;
            let tx = mx as usize + text_pad;
            blit_strip(buffer, buf_w, buf_h, &strip, tx, ty);
        }
    }
}

/// Blit a TextStrip's alpha-weighted pixels into the XRGB buffer.
fn blit_strip(
    buffer: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    strip: &TextStrip,
    tx: usize,
    ty: usize,
) {
    for (row, chunk) in strip.rgba.chunks_exact(strip.width * 4).enumerate() {
        let y = ty + row;
        if y >= buf_h {
            break;
        }
        for (col, px) in chunk.chunks_exact(4).enumerate() {
            let a = px[3] as u32;
            if a == 0 {
                continue;
            }
            let x = tx + col;
            if x >= buf_w {
                break;
            }
            let idx = y * buf_w + x;
            if idx >= buffer.len() {
                break;
            }
            let tr = px[0] as u32;
            let tg = px[1] as u32;
            let tb = px[2] as u32;
            let dst = buffer[idx];
            let dr = (dst >> 16) & 0xFF;
            let dg = (dst >> 8) & 0xFF;
            let db = dst & 0xFF;
            let nr = (tr * a + dr * (255 - a)) / 255;
            let ng = (tg * a + dg * (255 - a)) / 255;
            let nb = (tb * a + db * (255 - a)) / 255;
            buffer[idx] = (nr << 16) | (ng << 8) | nb;
        }
    }
}

/// A rendered text strip (RGBA) plus its dimensions.
struct TextStrip {
    rgba: Vec<u8>,
    width: usize,
    height: usize,
}

/// Render a single line of text into an RGBA buffer at the given size, using
/// the same blitz + Vello pipeline as page content. Returns `None` if the
/// rasterizer produces no content. The buffer is `width × height` RGBA8.
///
/// `scale` is the window scale factor; the strip is rendered at physical
/// resolution so text stays sharp on HiDPI displays.
fn render_text_strip(text: &str, width: usize, height: usize, scale: f32) -> Option<TextStrip> {
    render_text_strip_colored(text, width, height, scale, "#9aa5ce")
}

/// As [`render_text_strip`] but with a custom CSS color (e.g. "#d5d5e8").
fn render_text_strip_colored(
    text: &str,
    width: usize,
    height: usize,
    scale: f32,
    color: &str,
) -> Option<TextStrip> {
    use blitz_dom::DocumentConfig;
    use blitz_html::HtmlDocument;
    use blitz_traits::shell::Viewport;

    if width == 0 || height == 0 {
        return None;
    }
    // Render the text in a transparent document. The container is a fixed-height
    // block with overflow:hidden and white-space:nowrap, so the text never wraps
    // to a second line even if it is wider than the strip. All CSS values are in
    // CSS px; the viewport is sized in physical px with hidpi_scale so layout
    // matches the chrome bar. (height is physical px here — convert to CSS.)
    let css_h = (height as f32 / scale).max(8.0);
    let html = format!(
        "<!DOCTYPE html><html><head><style>\
         html,body{{margin:0;padding:0;background:transparent;overflow:hidden;\
         width:100%;height:100%;}}\
         .t{{font-family:system-ui,sans-serif;font-size:13px;color:{color};\
         white-space:nowrap;overflow:hidden;height:{h}px;line-height:{h}px;\
         display:block;padding:0;}}\
         </style></head><body><div class=\"t\">{}</div></body></html>",
        crate::browser::escape_html(text),
        h = css_h,
        color = color
    );
    let viewport = Viewport {
        window_size: (width as u32, height as u32),
        hidpi_scale: scale,
        ..Default::default()
    };
    let doc_config = DocumentConfig {
        viewport: Some(viewport),
        ..Default::default()
    };
    let mut doc = HtmlDocument::from_html(&html, doc_config);
    doc.resolve(0.0);

    let mut frame = crate::Frame::new(width as u32, height as u32);
    use anyrender::ImageRenderer;
    let mut renderer = anyrender_vello_cpu::VelloCpuImageRenderer::new(width as u32, height as u32);
    renderer.render(
        |scene| {
            blitz_paint::paint_scene(
                scene,
                &mut doc,
                scale as f64,
                width as u32,
                height as u32,
                0,
                0,
            );
        },
        &mut frame.rgba,
    );

    // Vello renders onto a black background by default; we want transparency so
    // the strip alpha-blends over the chrome bar. Recover alpha: any pixel the
    // rasterizer lit (non-zero color) is treated as opaque text, everything
    // else transparent. This is a heuristic but works for solid-color text.
    for chunk in frame.rgba.chunks_exact_mut(4) {
        let lit = chunk[0] != 0 || chunk[1] != 0 || chunk[2] != 0;
        // If lit, set alpha to 255 (the text color is already there). Else 0.
        if !lit {
            // Clear to transparent (premultiplied black stays black, alpha 0).
            chunk[3] = 0;
        } else {
            chunk[3] = 255;
        }
    }

    Some(TextStrip {
        rgba: frame.rgba,
        width,
        height,
    })
}

/// Run inline `<script>` blocks via Boa and splice any `document.write`
/// output into the HTML before it is parsed. SSR-style only: scripts execute
/// once at load; there is no interactive DOM binding yet.
#[cfg(feature = "js")]
fn run_scripts_ssr(html: &str) -> String {
    let result = aris_js::execute_scripts(html);
    for e in &result.errors {
        tracing::warn!("[js] {}", e);
    }
    if let Some(body) = result.document_props.get("body")
        && !body.is_empty()
    {
        return html.replace("</body>", &format!("{}\n</body>", body));
    }
    html.to_string()
}

/// Composite canvas 2D scenes onto the page render. Finds every `<canvas>`
/// element in the document, gets its layout position, and appends the
/// corresponding recorded Scene (from the thread_local CANVASES map) at that
/// position with the appropriate transform.
#[cfg(feature = "js")]
fn composite_canvases(
    scene: &mut impl anyrender::PaintScene,
    doc: &blitz_dom::BaseDocument,
    scale: f64,
) {
    use anyrender::PaintScene;
    use kurbo::Affine;

    // Find all <canvas> elements and their layout positions.
    for (node_id, node) in doc.tree().iter() {
        let is_canvas = node
            .element_data()
            .map(|e| format!("{:?}", e.name.local).contains("'canvas'"))
            .unwrap_or(false);
        if !is_canvas {
            continue;
        }
        let pos = node.absolute_position(0.0, 0.0);
        let w = node.final_layout.size.width;
        let h = node.final_layout.size.height;
        if w < 1.0 || h < 1.0 {
            continue;
        }
        // The canvas element's node id is the key into CANVASES.
        let cid = node_id as u32;
        crate::js_runtime::CANVASES.with(|cs| {
            let cs = cs.borrow();
            if let Some(canvas) = cs.get(&cid) {
                if canvas.has_content() {
                    let tf = Affine::translate((pos.x as f64 * scale, pos.y as f64 * scale))
                        * Affine::scale(scale);
                    scene.append_scene(canvas.scene().clone(), tf);
                }
            }
        });
    }
}

// ── Built-in pages ──────────────────────────────────────────

/// Built-in start page for new tabs.
fn start_page() -> String {
    "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"UTF-8\">\
     <title>aris — new tab</title>\
     <style>\
     body { margin:0; background:#1a1b26; font-family:system-ui,sans-serif; color:#a9b1d6; }\
     .wrap { max-width:640px; margin:80px auto; padding:0 24px; text-align:center; }\
     h1 { color:#7aa2f7; font-size:42px; margin:0 0 8px; }\
     p.sub { color:#565f89; margin:0 0 32px; }\
     .bookmarks { display:grid; grid-template-columns:repeat(2,1fr); gap:12px; }\
     a.card { display:block; background:#24283b; color:#c0caf5; text-decoration:none;\
              padding:16px; border-radius:10px; transition:background .15s; }\
     a.card:hover { background:#2f344d; }\
     .card .t { color:#7dcfff; font-weight:600; }\
     .card .d { color:#565f89; font-size:13px; margin-top:4px; }\
     .hint { color:#565f89; font-size:13px; margin-top:32px; }\
     kbd { background:#24283b; padding:2px 6px; border-radius:4px; color:#7dcfff; }\
     </style></head><body>\
     <div class=\"wrap\">\
       <h1>aris</h1>\
       <p class=\"sub\">a browser engine built on servo's pure-Rust front-end</p>\
       <div class=\"bookmarks\">\
         <a class=\"card\" href=\"https://example.com\"><span class=\"t\">example.com</span><div class=\"d\">test page</div></a>\
         <a class=\"card\" href=\"about:about\"><span class=\"t\">about:about</span><div class=\"d\">aris info</div></a>\
       </div>\
       <p class=\"hint\">Type a URL or search in the address bar, or press <kbd>Ctrl+L</kbd>.</p>\
     </div>\
     </body></html>"
        .to_string()
}

fn loading_page(url: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>Loading…</title>\
         <style>body{{font-family:system-ui,sans-serif;background:#1a1b26;color:#a9b1d6;padding:48px;text-align:center;}}\
         h1{{color:#7aa2f7;}}code{{color:#7dcfff;}}</style></head>\
         <body><h1>Loading…</h1><p><code>{}</code></p></body></html>",
        crate::browser::escape_html(url)
    )
}
