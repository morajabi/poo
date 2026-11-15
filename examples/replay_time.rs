//! cargo run --release --example replay_time -- <events-back> <root>...: times an FSEvents
//! replay over the given roots, starting that many event ids before now.
fn main() {
    let mut args = std::env::args().skip(1);
    let back: u64 = args.next().expect("events back").parse().unwrap();
    let roots: Vec<std::path::PathBuf> = args.map(std::path::PathBuf::from).collect();
    let since = poo::fsevents::current_id().saturating_sub(back);
    let t = std::time::Instant::now();
    let c = poo::fsevents::changes_since(&roots, since, std::time::Duration::from_secs(30)).expect("replay");
    println!("{:?} for {} changed folders over {:?}", t.elapsed(), c.paths.len(), roots);
}
