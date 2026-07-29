//! Tunable limits and parameters for searchlight.
//!
//! Every knob here should have a comment explaining what it's for and the
//! upper/lower bounds if applicable so an oncall engineer can adjust these
//! safely for searchlight if needed.
//!
//! When running locally, these knobs can all be overridden with an environment
//! variable.

use std::{
    path::PathBuf,
    sync::LazyLock,
};

use cmd_util::env::env_config;
// Knobs available in backend that are also available in searchlight.
#[allow(unused)]
pub use common::knobs::{
    ARCHIVE_FETCH_TIMEOUT_SECONDS,
    CODEL_QUEUE_CONGESTED_EXPIRATION_MILLIS,
    CODEL_QUEUE_IDLE_EXPIRATION_MILLIS,
    SEARCH_INDEX_COMPACTION_CONCURRENCY,
    VERTICAL_SCALING_CPU_COUNT,
    VERTICAL_SCALING_ENABLED,
    VERTICAL_SCALING_RESERVED_CPU_COUNT,
};

// Searchlight only knobs.

fn vertical_search_default(
    compatibility_default: usize,
    per_cpu: usize,
    minimum: usize,
    maximum: usize,
) -> usize {
    calculate_vertical_search_default(
        *VERTICAL_SCALING_ENABLED,
        *VERTICAL_SCALING_CPU_COUNT,
        *VERTICAL_SCALING_RESERVED_CPU_COUNT,
        compatibility_default,
        per_cpu,
        minimum,
        maximum,
    )
}

fn calculate_vertical_search_default(
    enabled: bool,
    cpu_count: usize,
    reserved_cpu_count: usize,
    compatibility_default: usize,
    per_cpu: usize,
    minimum: usize,
    maximum: usize,
) -> usize {
    if !enabled {
        return compatibility_default;
    }
    let cpu_derived = cpu_count
        .saturating_sub(reserved_cpu_count)
        .max(1)
        .saturating_mul(per_cpu)
        .clamp(minimum, maximum);
    // Enabling vertical scaling must not silently reduce a pool below the
    // established compatibility default on a medium-sized host.
    cpu_derived.max(compatibility_default.min(maximum))
}

/// Directory used by the in-process searcher's on-disk segment cache. An empty
/// value keeps the existing temporary-directory behavior.
pub static IN_PROCESS_SEARCH_CACHE_PATH: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let path = env_config("IN_PROCESS_SEARCH_CACHE_PATH", String::new());
    (!path.is_empty()).then(|| PathBuf::from(path))
});

/// Maximum size of the in-process searcher's on-disk segment cache.
pub static IN_PROCESS_SEARCH_CACHE_SIZE_BYTES: LazyLock<u64> = LazyLock::new(|| {
    env_config("IN_PROCESS_SEARCH_CACHE_SIZE_BYTES", bytesize::mib(500u64)).max(1)
});

/// Maximum number of general-purpose blocking search tasks running at once.
pub static SEARCH_GENERAL_POOL_MAX_CONCURRENCY: LazyLock<usize> = LazyLock::new(|| {
    env_config(
        "SEARCH_GENERAL_POOL_MAX_CONCURRENCY",
        vertical_search_default(50, 4, 16, 128),
    )
    .max(1)
});

/// Maximum number of queued general-purpose blocking search tasks.
pub static SEARCH_GENERAL_POOL_QUEUE_SIZE: LazyLock<usize> = LazyLock::new(|| {
    env_config(
        "SEARCH_GENERAL_POOL_QUEUE_SIZE",
        if *VERTICAL_SCALING_ENABLED {
            SEARCH_GENERAL_POOL_MAX_CONCURRENCY.saturating_mul(20)
        } else {
            1000
        },
    )
    .max(1)
});

/// The maximum number of compactions we can run concurrently on one
/// searchlight instance. Each compaction takes 4 cores, so this should
/// always be less than the number of cores on the machine / 4 to reserve CPU
/// for searches.
///
/// The queue size for compactions is set to QUEUE_SIZE_MULTIPLIER * this
/// number, so this knob also determines the maximum queue length.
pub static MAX_CONCURRENT_SEGMENT_COMPACTIONS: LazyLock<usize> = LazyLock::new(|| {
    env_config(
        "MAX_CONCURRENT_SEGMENT_COMPACTIONS",
        *SEARCH_INDEX_COMPACTION_CONCURRENCY,
    )
    .max(1)
});

/// The maximum number of segments we can fetch in parallel across all
/// searches and compactions.
///
/// NOTE: You must consider the cache timeout, the maximum segment size and
/// the serial disk write speed of searchlight before changing this number.
/// If you set this number too high, we will not be able to download
/// segments fast enough and will have congestion collapse.
///
/// A rough way to calculate the maximum value for this knob is to determine the
/// amount of time it takes to download N segments at their maximum size:
///
/// max segment size (~3.2 GiB) * concurrent fetches / max write throughput
/// speed (~600 MiB/s)
///
/// Then compare that to the cache timeout seconds (120s) and ensure that the
/// time to fetch segments is well under the timeout. If we exceed the timeout,
/// then we'll have congestion collapse because we will fail to make progress
/// downloading segments.
///
/// The queue size for fetches is set to QUEUE_SIZE_MULTIPLIER * this number, so
/// this knob also determines the maximum queue length.
pub static MAX_CONCURRENT_SEGMENT_FETCHES: LazyLock<usize> = LazyLock::new(|| {
    env_config(
        "MAX_CONCURRENT_SEGMENT_FETCHES",
        vertical_search_default(8, 1, 8, 16),
    )
    .max(1)
});

/// The maximum number of concurrent vector searches we'll run at once,
/// based on a very rough estimate of memory used per search.
///
/// The queue size for searches is set to QUEUE_SIZE_MULTIPLIER * this number,
/// so this knob also determines the maximum queue length.
pub static MAX_CONCURRENT_VECTOR_SEARCHES: LazyLock<usize> = LazyLock::new(|| {
    env_config(
        "MAX_CONCURRENT_VECTOR_SEARCHES",
        vertical_search_default(20, 2, 8, 64),
    )
    .max(1)
});

/// A generic multiplier applied to concurrencly limits for most pools in
/// searchlight to figure out the queue size.
pub static QUEUE_SIZE_MULTIPLIER: LazyLock<usize> =
    LazyLock::new(|| env_config("QUEUE_SIZE_MULTIPLIER", 20));

/// Fraction (0.0 - 1.0) of the total archive cache a single index or
/// deployment must exceed before we emit the archive usage gauges. Set lower to
/// see more metrics and higher to suppress noisy low-volume entries.
pub static ARCHIVE_METRIC_EMIT_THRESHOLD_FRACTION: LazyLock<f64> =
    LazyLock::new(|| env_config("ARCHIVE_METRIC_EMIT_THRESHOLD_FRACTION", 0.05));

/// The maximum number of qdrant Segments (each backed by a RocksDB
/// instance) that we'll keep in memory in the LRU at once.
/// See https://www.notion.so/convex-dev/Vector-Search-Scaling-Issues-0e7c2dde6ea241af828c89a77c593f64?pvs=4#2b1852e44b734362a1b05b6dec62b744
/// for where this default value comes from. The actual value here may be some
/// multiple of the value in the doc depending on the amount of memory in the
/// instance type we're currently using for searchlight.
pub static MAX_VECTOR_LRU_SIZE: LazyLock<u64> =
    LazyLock::new(|| env_config("MAX_VECTOR_LRU_ENTRIES", 120));

/// The maximum number of segments we're allowed to prefetch at one time in a
/// given searchlight node.
pub static MAX_CONCURRENT_VECTOR_SEGMENT_PREFETCHES: LazyLock<usize> = LazyLock::new(|| {
    env_config(
        "MAX_CONCURRENT_VECTOR_PREFETCHES",
        if *VERTICAL_SCALING_ENABLED {
            VERTICAL_SCALING_CPU_COUNT
                .saturating_sub(*VERTICAL_SCALING_RESERVED_CPU_COUNT)
                .max(1)
                .div_ceil(4)
                .clamp(2, 8)
        } else {
            2
        },
    )
    .max(1)
});
/// The maximum number of text segments (each backed by a single-segment tantiy
/// index ) that we'll keep in memory in the LRU at once.
pub static MAX_TEXT_LRU_ENTRIES: LazyLock<u64> =
    LazyLock::new(|| env_config("MAX_TEXT_LRU_ENTRIES", 120));

/// The maximum number of concurrent text searches we'll run at once,
/// based on a very rough estimate of memory used per search.
///
/// The queue size for searches is set to QUEUE_SIZE_MULTIPLIER * this number,
/// so this knob also determines the maximum queue length.
pub static MAX_CONCURRENT_TEXT_SEARCHES: LazyLock<usize> = LazyLock::new(|| {
    env_config(
        "MAX_CONCURRENT_TEXT_SEARCHES",
        vertical_search_default(20, 4, 16, 128),
    )
    .max(1)
});

#[cfg(test)]
mod tests {
    use super::calculate_vertical_search_default;

    #[test]
    fn compatibility_search_default_is_unchanged() {
        assert_eq!(
            calculate_vertical_search_default(false, 64, 8, 20, 4, 16, 128),
            20
        );
    }

    #[test]
    fn search_default_uses_unreserved_cpus_and_bounds() {
        assert_eq!(
            calculate_vertical_search_default(true, 10, 1, 20, 4, 16, 128),
            36
        );
        assert_eq!(
            calculate_vertical_search_default(true, 512, 1, 20, 4, 16, 128),
            128
        );
    }

    #[test]
    fn vertical_search_default_never_regresses_compatibility_capacity() {
        assert_eq!(
            calculate_vertical_search_default(true, 10, 1, 50, 4, 16, 128),
            50
        );
        assert_eq!(
            calculate_vertical_search_default(true, 10, 1, 20, 2, 8, 64),
            20
        );
    }
}
