mod in_process;
mod metrics;
#[allow(clippy::module_inception)]
mod searcher;
mod searchlight_knobs;
mod segment_cache;

pub use in_process::{
    InProcessSearcher,
    SearcherStub,
};
pub use searcher::{
    Bm25Stats,
    FieldDeletions,
    FragmentedTextStorageKeys,
    PostingListMatch,
    PostingListQuery,
    Searcher,
    SearcherImpl,
    SegmentTermMetadataFetcher,
    Term,
    TermDeletionsByField,
    TermValue,
    TokenMatch,
    TokenQuery,
};
pub(crate) use searchlight_knobs::ARCHIVE_METRIC_EMIT_THRESHOLD_FRACTION;
pub use searchlight_knobs::{
    MAX_CONCURRENT_SEGMENT_COMPACTIONS,
    MAX_CONCURRENT_SEGMENT_FETCHES,
    MAX_CONCURRENT_TEXT_SEARCHES,
    MAX_CONCURRENT_VECTOR_SEARCHES,
    MAX_CONCURRENT_VECTOR_SEGMENT_PREFETCHES,
    SEARCH_GENERAL_POOL_MAX_CONCURRENCY,
    SEARCH_GENERAL_POOL_QUEUE_SIZE,
};
pub use text_search::tracker::SegmentTermMetadata;
