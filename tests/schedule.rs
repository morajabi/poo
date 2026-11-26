//! When schedules run. launchd wakes poo every 15 minutes, out of step with a schedule's
//! HH:MM and not at all while the Mac sleeps, so a daily slot has to stay due until a tick
//! catches it. Times are local; the dates sit in mid-January, clear of daylight saving changes.

use poo::config::Schedule;
use poo::schedule::{due_at, presets, records, RunRecord};

const ADDED: i64 = 1_768_478_400; // 2026-01-15 12:00 UTC

fn preset(id: &str) -> Schedule {
    presets().into_iter().find(|(p, _, _)| *p == id).map(|(_, _, s)| s).unwrap()
}

fn due(s: &Schedule, last: Option<i64>, now: i64) -> bool {
    now >= due_at(s, last, ADDED, now)
}

#[test]
fn nightly_runs_at_the_first_tick_after_its_time() {
    let nightly = preset("nightly");
    // The first 03:30 after the schedule was added.
    let slot = due_at(&nightly, None, ADDED, ADDED);
    assert!(slot > ADDED && slot - ADDED <= 86400, "{} is not the next 03:30", slot);

    assert!(!due(&nightly, None, slot - 12 * 60), "03:18 is too early");
    assert!(due(&nightly, None, slot + 3 * 60), "the 03:33 tick runs it");
    assert!(due(&nightly, None, slot + 5 * 3600 + 30 * 60), "asleep at 03:30, it runs on wake");

    // After running at 03:33 the next run is the next night's 03:30, and its 03:33 tick runs it.
    let ran = slot + 3 * 60;
    assert_eq!(due_at(&nightly, Some(ran), ADDED, ran), slot + 86400);
    assert!(!due(&nightly, Some(ran), slot + 86400 - 12 * 60));
    assert!(due(&nightly, Some(ran), slot + 86400 + 3 * 60));

    // A late run (woke at 09:00) does not push the next night out.
    let late = slot + 5 * 3600 + 30 * 60;
    assert_eq!(due_at(&nightly, Some(late), ADDED, late), slot + 86400);
}

#[test]
fn weekly_waits_a_week_from_the_slot_it_served() {
    let weekly = preset("weekly");
    let slot = due_at(&weekly, None, ADDED, ADDED);
    let ran = slot + 10 * 60;
    assert_eq!(due_at(&weekly, Some(ran), ADDED, ran), slot + 7 * 86400);
    assert!(!due(&weekly, Some(ran), slot + 6 * 86400 + 3 * 60));
    assert!(due(&weekly, Some(ran), slot + 7 * 86400 + 3 * 60));
}

#[test]
fn interval_schedules_run_every_interval() {
    let guard = preset("guard");
    assert!(due(&guard, None, ADDED), "never ran: due now");
    assert_eq!(due_at(&guard, Some(ADDED), ADDED, ADDED), ADDED + 900);
}

#[test]
fn which_runs_replace_the_last_record() {
    let rec = |status: &str| RunRecord { status: status.into(), ..Default::default() };
    assert!(records(None, &rec("not-needed")), "the first check is recorded");
    assert!(!records(Some(&rec("ran")), &rec("not-needed")), "a check keeps the last real run");
    assert!(records(Some(&rec("error")), &rec("not-needed")), "an old error does not stay the state forever");
    assert!(!records(Some(&rec("error")), &rec("busy")), "busy tries again next tick");
    assert!(records(Some(&rec("ran")), &rec("ran")));
    assert!(records(Some(&rec("ran")), &rec("error")));
}
