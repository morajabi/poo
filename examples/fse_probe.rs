//! cargo run --release --example fse_probe -- <dir>: replays FSEvents for <dir> since an id
//! taken before touching a file in it.
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("dir"));
    let id = poo::fsevents::current_id();
    std::thread::sleep(std::time::Duration::from_millis(200));
    std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
    std::fs::write(dir.join("sub/deeper/file.txt"), b"hi").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let t = std::time::Instant::now();
    let c = poo::fsevents::changes_since(&[dir.clone()], id, std::time::Duration::from_secs(10)).expect("replay");
    println!("replay took {:?}; lost={} rescan={:?}", t.elapsed(), c.lost, c.rescan_dirs);
    for p in c.paths {
        println!("  {}", p.display());
    }
}
