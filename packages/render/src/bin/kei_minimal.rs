// Minimal test: just write to stderr and exit
fn main() {
    real_main();
}

#[cfg(not(unix))]
fn real_main() {
    eprintln!("kei_minimal: unix-only binary; nothing to do on this host");
}

#[cfg(unix)]
fn real_main() {
    // Write directly to fd 2 using raw syscall to avoid any Rust std overhead
    let msg = b"HELLO FROM KEI_UI\n";
    unsafe {
        let _ = libc::write(2, msg.as_ptr() as *const libc::c_void, msg.len() as u32);
    }
    // Exit immediately
    std::process::exit(0);
}
