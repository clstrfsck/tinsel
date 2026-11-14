use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

/// Applies `f` to the indices `0..len` on at most `jobs` scoped worker
/// threads. Results keep index order, and a panic fails only its task.
pub fn map<T, F>(len: usize, jobs: usize, f: F) -> Vec<Result<T, ()>>
where
    T: Send,
    F: Fn(usize) -> T + Sync,
{
    if len == 0 {
        return Vec::new();
    }
    let jobs = jobs.clamp(1, len);
    let next = AtomicUsize::new(0);
    let (send, recv) = mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            let send = send.clone();
            let f = &f;
            let next = &next;
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= len {
                        break;
                    }
                    let result = catch_unwind(AssertUnwindSafe(|| f(i))).map_err(|_| ());
                    if send.send((i, result)).is_err() {
                        break;
                    }
                }
            });
        }
    });
    drop(send);

    let mut results: Vec<Option<Result<T, ()>>> =
        std::iter::repeat_with(|| None).take(len).collect();
    for (i, result) in recv {
        results[i] = Some(result);
    }
    results
        .into_iter()
        .map(|result| result.unwrap_or(Err(())))
        .collect()
}
