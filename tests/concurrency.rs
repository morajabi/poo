//! One clean at a time: a second clean waits for the first, says what it waits for, gives up
//! after its timeout and stops waiting when cancelled. One test: env vars are process-wide.
#![cfg(unix)]

use poo::schedule::{busy_message, holder, RunLock};
use std::time::{Duration, Instant};

#[test]
fn run_lock_waits_names_the_holder_and_gives_up() {
    let base = std::env::temp_dir().join(format!("poo-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::env::set_var("POO_HOME", &base);

    let held = RunLock::try_acquire("schedule:guard").expect("first lock");
    assert!(RunLock::try_acquire("cli").is_none(), "a second clean must not run alongside");
    assert_eq!(holder().unwrap().actor, "schedule:guard");
    assert!(busy_message().contains("scheduled clean `guard`"), "{}", busy_message());

    // Gives up after its timeout and says why.
    let t = Instant::now();
    let err = RunLock::acquire_wait("cli", Duration::from_millis(400), &|_| {}, &|| false).err().expect("still held");
    assert!(t.elapsed() >= Duration::from_millis(400));
    assert!(!err.cancelled);
    assert!(err.message().contains("guard"), "{}", err.message());

    // Cancel stops the wait.
    let err = RunLock::acquire_wait("cli", Duration::from_secs(30), &|_| {}, &|| true).err().expect("cancelled");
    assert!(err.cancelled);

    // Waits for the holder to finish, reporting while it waits.
    let notes = std::sync::atomic::AtomicUsize::new(0);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1200));
        drop(held);
    });
    let got = RunLock::acquire_wait(
        "app",
        Duration::from_secs(10),
        &|h| {
            assert_eq!(h.actor, "schedule:guard");
            notes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
        &|| false,
    );
    releaser.join().unwrap();
    assert!(got.is_ok(), "the lock was released, the wait must end");
    assert!(notes.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    assert_eq!(holder().unwrap().actor, "app", "the new holder writes itself in");
    let _ = std::fs::remove_dir_all(&base);
}
