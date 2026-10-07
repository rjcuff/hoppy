//! Run many slow network calls at once on a fixed number of threads.

use std::sync::{Mutex, PoisonError};
use std::thread;

/// Apply `work` to every item using up to `workers` threads. Results come
/// back in the same order as the items went in.
pub fn map<T, R>(items: Vec<T>, workers: usize, work: impl Fn(T) -> R + Sync) -> Vec<R>
where
    T: Send,
    R: Send,
{
    let total = items.len();
    let queue = Mutex::new(items.into_iter().enumerate());
    let done = Mutex::new(Vec::with_capacity(total));

    thread::scope(|scope| {
        for _ in 0..workers.clamp(1, total.max(1)) {
            scope.spawn(|| {
                loop {
                    // The lock is held only long enough to take the next item.
                    let next = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
                    let Some((index, item)) = next else { break };
                    let result = work(item);
                    done.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((index, result));
                }
            });
        }
    });

    let mut done = done.into_inner().unwrap_or_else(PoisonError::into_inner);
    done.sort_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_input_order() {
        let doubled = map((0..200).collect(), 16, |n: u32| n * 2);
        let expected: Vec<u32> = (0..200).map(|n| n * 2).collect();
        assert_eq!(doubled, expected);
    }

    #[test]
    fn handles_empty_input_and_zero_workers() {
        assert!(map(Vec::<u8>::new(), 8, |n| n).is_empty());
        assert_eq!(map(vec![1, 2, 3], 0, |n: u8| n + 1), [2, 3, 4]);
    }
}
