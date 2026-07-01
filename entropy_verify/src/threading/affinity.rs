/// CPU core pinning (affinity) for worker threads.
///
/// Pins each worker thread to a dedicated CPU core to maximize L1/L2 cache
/// locality, minimize context switching, and prevent the OS scheduler from
/// migrating I/O-hot threads between cores.
///
/// Core 0 is reserved for the TUI render loop by default.

/// Pin the current thread to the given core index.
///
/// Returns `true` if pinning succeeded, `false` if it failed (non-fatal).
pub fn pin_to_core(core_index: usize) -> bool {
    let core_ids = match core_affinity::get_core_ids() {
        Some(ids) => ids,
        None => return false,
    };

    if core_index >= core_ids.len() {
        return false;
    }

    core_affinity::set_for_current(core_ids[core_index])
}

/// Get the number of available CPU cores.
pub fn available_cores() -> usize {
    core_affinity::get_core_ids()
        .map(|ids| ids.len())
        .unwrap_or_else(|| num_cpus::get_physical())
}

/// Calculate the core assignment for a worker thread.
///
/// Workers are assigned to cores starting from core 1 (core 0 is reserved
/// for the TUI). If there are more workers than cores, workers wrap around.
pub fn core_for_worker(worker_index: usize, total_cores: usize) -> usize {
    if total_cores <= 1 {
        return 0; // Only one core available
    }
    // Skip core 0, assign workers to cores 1..N in round-robin.
    1 + (worker_index % (total_cores - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_assignment() {
        assert_eq!(core_for_worker(0, 8), 1);
        assert_eq!(core_for_worker(1, 8), 2);
        assert_eq!(core_for_worker(6, 8), 7);
        assert_eq!(core_for_worker(7, 8), 1); // wraps around
    }

    #[test]
    fn test_single_core() {
        assert_eq!(core_for_worker(0, 1), 0);
        assert_eq!(core_for_worker(5, 1), 0);
    }
}
