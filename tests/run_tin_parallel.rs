mod support;

use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn parallel_map_runs_every_task_once_and_keeps_order() {
    let seen: Vec<AtomicUsize> = (0..20).map(|_| AtomicUsize::new(0)).collect();
    let results = support::parallel::map(seen.len(), 4, |i| {
        seen[i].fetch_add(1, Ordering::Relaxed);
        i * i
    });

    assert_eq!(results, (0..20).map(|i| Ok(i * i)).collect::<Vec<_>>());
    assert!(seen.iter().all(|n| n.load(Ordering::Relaxed) == 1));
}

#[test]
fn parallel_map_contains_a_panic_and_runs_later_tasks() {
    let seen: Vec<AtomicUsize> = (0..8).map(|_| AtomicUsize::new(0)).collect();
    let results = support::parallel::map(seen.len(), 2, |i| {
        seen[i].fetch_add(1, Ordering::Relaxed);
        assert_ne!(i, 2, "task panic");
        i
    });

    assert_eq!(results[2], Err(()));
    assert_eq!(results[7], Ok(7));
    assert!(seen.iter().all(|n| n.load(Ordering::Relaxed) == 1));
}
