//! Test-binary-only fault injection. No environment hooks or branches enter production builds.
use std::{
    io::{self, Write},
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
#[derive(Clone, Copy, Debug)]
pub(super) enum Action {
    Stop,
    NoSpace,
    Io,
}
struct Plan {
    point: String,
    occurrence: usize,
    seen: AtomicUsize,
    action: Action,
}
static PLAN: OnceLock<Plan> = OnceLock::new();
pub(super) fn arm(point: String, occurrence: usize, action: Action) {
    assert!(occurrence > 0);
    assert!(
        matches!(action, Action::Stop) || point != "manifest.dir_synced",
        "no fallible I/O follows the completed manifest synchronization"
    );
    assert!(
        PLAN.set(Plan {
            point,
            occurrence,
            seen: AtomicUsize::new(0),
            action
        })
        .is_ok(),
        "one fault per worker"
    );
}
pub(super) fn fired() -> bool {
    PLAN.get()
        .is_some_and(|plan| plan.seen.load(Ordering::Acquire) >= plan.occurrence)
}
pub(super) fn hit(point: &str) -> io::Result<()> {
    let Some(plan) = PLAN.get().filter(|plan| plan.point == point) else {
        return Ok(());
    };
    let seen = plan.seen.fetch_add(1, Ordering::AcqRel) + 1;
    if seen != plan.occurrence {
        return Ok(());
    }
    eprintln!("MYNOFY_STORAGE_POINT {point} {seen}");
    io::stderr().flush()?;
    match plan.action {
        Action::Stop => loop {
            std::thread::park();
        },
        Action::NoSpace => Err(io::Error::from_raw_os_error(
            rustix::io::Errno::NOSPC.raw_os_error(),
        )),
        Action::Io => Err(io::Error::from_raw_os_error(
            rustix::io::Errno::IO.raw_os_error(),
        )),
    }
}
pub(super) fn phase(target: &str, phase: &str) -> io::Result<()> {
    let kind = if target == "state.bin" {
        "manifest"
    } else if target == "references.bin" {
        "references"
    } else {
        "topic"
    };
    hit(&format!("{kind}.{phase}"))
}
