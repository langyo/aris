// kei_tty — aris-rendered vtty display + WS JSON-RPC gateway.
//
// Architecture:
//   1. The vtty frame is a Linux-kernel-console-style status screen
//      (black background, light monospace text, `[    0.000000] kei: ...`
//      kernel-log layout). It is pre-rendered on the HOST by
//      `prerender_vtty` through the full aris-render pipeline
//      (Blitz DOM + Stylo CSS + Taffy layout + parley text + Vello CPU)
//      and tracked as `packages/render/assets/vtty_console.png`.
//      This binary embeds that PNG, decodes it at runtime (pure-Rust
//      `png` crate — no system libs) and blits it to /dev/fb0.
//   2. WS JSON-RPC server on port 8423 for the headless HMI (webui).
//   3. Standby poll loop (mouse + WS connections).
//
// Regenerate the console frame after style changes:
//   cargo run -p aris-render --no-default-features --features render \
//       --bin prerender_vtty

#![allow(unsafe_code)]

/// Pre-rendered vtty console frame (PNG, RGBA8, 1280x800).
#[allow(dead_code)] // used only in the unix fbdev path
const VTTY_PNG: &[u8] = include_bytes!("../../assets/vtty_console.png");

fn main() {
    real_main();
}

#[cfg(not(unix))]
fn real_main() {
    eprintln!("kei_tty: unix-only binary (kei /dev/fb0 target); nothing to do on this host");
}

#[cfg(unix)]
fn real_main() {
    let log = |m: &[u8]| unsafe { libc::write(2, m.as_ptr() as *const _, m.len() as _); };
    log(b"kei_tty: aris-rendered vtty + gateway starting\n");

    // Decode the embedded console PNG (host pre-rendered by prerender_vtty).
    let (width, height, rgba) = match decode_png_rgba(VTTY_PNG) {
        Some(frame) => frame,
        None => {
            log(b"kei_tty: FATAL: embedded vtty PNG decode failed\n");
            loop { std::thread::sleep(std::time::Duration::from_secs(3600)); }
        }
    };
    let dims = format!("kei_tty: console frame {}x{}\n", width, height);
    log(dims.as_bytes());

    let fb_path = std::env::var("KEI_FB").unwrap_or_else(|_| "/dev/fb0".to_string());
    if std::path::Path::new(&fb_path).exists() {
        log(b"kei_tty: writing aris vtty to /dev/fb0\n");
        let npix = width * height;
        let mut bgrx = vec![0u8; npix * 4];
        for i in 0..npix {
            let s = i * 4;
            let d = i * 4;
            bgrx[d] = rgba[s + 2];
            bgrx[d + 1] = rgba[s + 1];
            bgrx[d + 2] = rgba[s];
            bgrx[d + 3] = 0xFF;
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().read(true).write(true).open(&fb_path) {
            use std::io::{Seek, Write};
            let _ = file.seek(std::io::SeekFrom::Start(0));
            const CHUNK: usize = 8192;
            let mut wr = 0usize;
            let sz = bgrx.len();
            while wr < sz {
                let end = (wr + CHUNK).min(sz);
                if file.write(&bgrx[wr..end]).unwrap_or(0) == 0 { break; }
                wr += end - wr;
            }
            unsafe {
                const FB_FLUSH: u64 = 0x4606;
                let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
                for _ in 0..65 { let _ = libc::ioctl(fd, FB_FLUSH as _, 0usize); }
            }
            log(b"kei_tty: vtty displayed\n");
        }
    } else {
        log(b"kei_tty: no fb device (set KEI_FB); skipping display\n");
    }

    let ws_fd = create_tcp_listener(8423);
    if ws_fd >= 0 { log(b"kei_tty: WS listening on :8423\n"); }

    let mut mouse_fd: i32 = -1;
    for i in 0..8u32 {
        let path = format!("/dev/input/event{}", i);
        if let Ok(fd) = open_dev(&path) {
            let name = get_dev_name(fd);
            if name.contains("Mouse") { mouse_fd = fd; break; }
            unsafe { libc::close(fd); }
        }
    }

    log(b"kei_tty: entering standby\n");
    let mut event_buf = [0u8; 24 * 8];
    loop {
        let mut fds = [libc::pollfd { fd: -1, events: 0, revents: 0 }; 3];
        let mut n: u64 = 0;
        if mouse_fd >= 0 { fds[n as usize] = libc::pollfd { fd: mouse_fd, events: libc::POLLIN, revents: 0 }; n += 1; }
        if ws_fd >= 0 { fds[n as usize] = libc::pollfd { fd: ws_fd, events: libc::POLLIN, revents: 0 }; n += 1; }
        if n == 0 { std::thread::sleep(std::time::Duration::from_secs(1)); continue; }
        if unsafe { libc::poll(fds.as_mut_ptr(), n, 500) } <= 0 { continue; }
        if mouse_fd >= 0 && fds[0].revents & libc::POLLIN != 0 {
            unsafe { libc::read(mouse_fd, event_buf.as_mut_ptr() as *mut _, event_buf.len()); }
        }
        let ws_idx = if mouse_fd >= 0 { 1 } else { 0 };
        if ws_fd >= 0 && ws_idx < n as usize && fds[ws_idx].revents & libc::POLLIN != 0 {
            let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
            let mut alen: libc::socklen_t = std::mem::size_of::<libc::sockaddr_in>() as u32;
            let cfd = unsafe { libc::accept(ws_fd, &mut addr as *mut _ as *mut _, &mut alen as *mut _) };
            if cfd >= 0 { log(b"kei_tty: WS client\n"); serve_ws_client(cfd); }
        }
    }
}

/// Decode an embedded PNG into (width, height, RGBA8 pixels).
#[cfg(unix)]
fn decode_png_rgba(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut decoder = png::Decoder::new(data);
    decoder.set_transformations(png::Transformations::RGBA | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    Some((info.width as usize, info.height as usize, buf))
}

#[cfg(unix)]
fn open_dev(path: &str) -> std::io::Result<i32> {
    let c = std::ffi::CString::new(path).map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
    if fd < 0 { Err(std::io::Error::last_os_error()) } else { Ok(fd) }
}

#[cfg(unix)]
fn get_dev_name(fd: i32) -> String {
    let mut buf = [0u8; 256];
    let ret = unsafe { libc::ioctl(fd, 0x81004506u64 as _, buf.as_mut_ptr()) };
    if ret >= 0 { let len = buf.iter().position(|&b| b == 0).unwrap_or(256); String::from_utf8_lossy(&buf[..len]).to_string() }
    else { String::new() }
}

#[cfg(unix)]
fn create_tcp_listener(port: u16) -> i32 {
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if fd < 0 { return -1; }
        let opt: i32 = 1;
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &opt as *const _ as *const _, 4);
        let addr = libc::sockaddr_in { sin_family: libc::AF_INET as u16, sin_port: port.to_be(), sin_addr: libc::in_addr { s_addr: 0 }, sin_zero: [0; 8] };
        if libc::bind(fd, &addr as *const _ as *const _, 16) < 0 { libc::close(fd); return -1; }
        if libc::listen(fd, 4) < 0 { libc::close(fd); return -1; }
        let fl = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        fd
    }
}

#[cfg(unix)]
fn serve_ws_client(fd: i32) {
    unsafe { let fl = libc::fcntl(fd, libc::F_GETFL); libc::fcntl(fd, libc::F_SETFL, fl & !libc::O_NONBLOCK); }
    let mut buf = [0u8; 4096];
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
    if n <= 0 { unsafe { libc::close(fd); } return; }
    let req = String::from_utf8_lossy(&buf[..n as usize]);
    let key = extract_key(&req);
    if key.is_empty() { unsafe { libc::close(fd); } return; }
    let resp = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", compute_accept(&key));
    unsafe { libc::write(fd, resp.as_ptr() as *const _, resp.len()); }

    loop {
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, 2000) } <= 0 { continue; }
        let mut fb = [0u8; 4096];
        let n2 = ws_read(fd, &mut fb);
        if n2 <= 0 { break; }
        let msg = String::from_utf8_lossy(&fb[..n2 as usize]);
        let reply: String = if msg.contains("Kei.Ping") {
            r#"{"jsonrpc":"2.0","id":"1","result":{"pong":true}}"#.into()
        } else if msg.contains("Kei.Status") {
            r#"{"jsonrpc":"2.0","id":"1","result":{"status":"ready","version":"0.1.0","arch":"aarch64","mode":"gateway","display":"aris-vtty"}}"#.into()
        } else if msg.contains("Base.Heartbeat") {
            r#"{"jsonrpc":"2.0","id":"1","result":{"ok":true}}"#.into()
        } else {
            r#"{"jsonrpc":"2.0","id":"1","error":{"code":-32601,"message":"method not found"}}"#.into()
        };
        ws_write(fd, reply.as_bytes());
    }
    unsafe { libc::close(fd); }
}

#[cfg(unix)]
fn ws_read(fd: i32, buf: &mut [u8]) -> i32 {
    let mut hdr = [0u8; 2];
    if unsafe { libc::read(fd, hdr.as_mut_ptr() as _, 2) } < 2 { return -1; }
    let op = hdr[0] & 0x0F;
    let masked = (hdr[1] & 0x80) != 0;
    let mut pl = (hdr[1] & 0x7F) as usize;
    if pl == 126 { let mut e = [0u8; 2]; unsafe { libc::read(fd, e.as_mut_ptr() as _, 2); } pl = u16::from_be_bytes(e) as usize; }
    else if pl == 127 { let mut e = [0u8; 8]; unsafe { libc::read(fd, e.as_mut_ptr() as _, 8); } pl = u64::from_be_bytes(e) as usize; }
    let mut mask = [0u8; 4];
    if masked { unsafe { libc::read(fd, mask.as_mut_ptr() as _, 4); } }
    if pl > buf.len() { return -1; }
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as _, pl) };
    if n < 0 { return -1; }
    if masked { for i in 0..n as usize { buf[i] ^= mask[i % 4]; } }
    if op == 8 { return -1; }
    n as i32
}

#[cfg(unix)]
fn ws_write(fd: i32, data: &[u8]) {
    let mut hdr = vec![0x81u8];
    if data.len() < 126 { hdr.push(data.len() as u8); }
    else if data.len() < 65536 { hdr.push(126); hdr.extend_from_slice(&(data.len() as u16).to_be_bytes()); }
    else { hdr.push(127); hdr.extend_from_slice(&(data.len() as u64).to_be_bytes()); }
    unsafe { libc::write(fd, hdr.as_ptr() as _, hdr.len()); libc::write(fd, data.as_ptr() as _, data.len()); }
}

#[cfg(unix)]
fn extract_key(req: &str) -> String {
    for l in req.lines() { if l.trim().to_lowercase().starts_with("sec-websocket-key:") { return l[18..].trim().to_string(); } }
    String::new()
}

#[cfg(unix)]
fn compute_accept(key: &str) -> String {
    let chk = format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", key);
    let hash = sha1(chk.as_bytes());
    let mut b64 = String::new();
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in hash.chunks(3) {
        let b = [chunk.get(0).copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        b64.push(T[((n>>18)&63) as usize] as char);
        b64.push(T[((n>>12)&63) as usize] as char);
        b64.push(if chunk.len()>1 { T[((n>>6)&63) as usize] } else { b'=' } as char);
        b64.push(if chunk.len()>2 { T[(n&63) as usize] } else { b'=' } as char);
    }
    b64
}

#[cfg(unix)]
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bl = (data.len() as u64) * 8;
    let mut msg = data.to_vec(); msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&bl.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, wd) in chunk.chunks(4).enumerate() { w[i] = u32::from_be_bytes([wd[0],wd[1],wd[2],wd[3]]); }
        for i in 16..80 { w[i] = (w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16]).rotate_left(1); }
        let (mut a,mut b,mut c,mut d,mut e) = (h[0],h[1],h[2],h[3],h[4]);
        for i in 0..80 {
            let (f,k) = if i<20 { ((b&c)|(!b&d), 0x5A827999) } else if i<40 { (b^c^d, 0x6ED9EBA1) } else if i<60 { ((b&c)|(b&d)|(c&d), 0x8F1BBCDC) } else { (b^c^d, 0xCA62C1D6) };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[i]);
            e=d; d=c; c=b.rotate_left(30); b=a; a=t;
        }
        h[0]=h[0].wrapping_add(a); h[1]=h[1].wrapping_add(b); h[2]=h[2].wrapping_add(c); h[3]=h[3].wrapping_add(d); h[4]=h[4].wrapping_add(e);
    }
    let mut r = [0u8; 20];
    r[0..4].copy_from_slice(&h[0].to_be_bytes()); r[4..8].copy_from_slice(&h[1].to_be_bytes());
    r[8..12].copy_from_slice(&h[2].to_be_bytes()); r[12..16].copy_from_slice(&h[3].to_be_bytes());
    r[16..20].copy_from_slice(&h[4].to_be_bytes());
    r
}
