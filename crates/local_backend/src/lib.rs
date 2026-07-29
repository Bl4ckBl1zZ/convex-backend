#![feature(try_blocks)]
#![feature(try_blocks_heterogeneous)]
#![feature(iterator_try_collect)]
#![feature(coroutines)]
#![feature(exhaustive_patterns)]

use std::{
    self,
    sync::Arc,
    time::Duration,
};

use ::authentication::{
    access_token_auth::NullAccessTokenAuth,
    application_auth::ApplicationAuth,
};
use application::{
    self,
    api::ApplicationApi,
    log_visibility::RedactLogsToClient,
    usage_limits::NoopUsageLimitNotifier,
    Application,
    QueryCache,
};
use common::{
    self,
    http::{
        fetch::ProxiedFetchClient,
        RouteMapper,
    },
    knobs::{
        APPLICATION_MAX_CONCURRENT_MUTATIONS,
        APPLICATION_MAX_CONCURRENT_NODE_ACTIONS,
        APPLICATION_MAX_CONCURRENT_QUERIES,
        APPLICATION_MAX_CONCURRENT_V8_ACTIONS,
        ASYNC_JOIN_CONCURRENCY,
        COMMITTER_MAX_CONCURRENT_PERSISTENCE_WRITES,
        DOCUMENT_RETENTION_RATE_LIMIT,
        FUNRUN_ISOLATE_ACTIVE_THREADS,
        INDEX_CACHE_SIZE,
        INDEX_RANGE_BATCH_CONCURRENCY,
        IN_MEMORY_INDEX_LOAD_CONCURRENCY,
        MAX_ACTION_ISOLATE_WORKERS,
        MAX_TRANSACTION_ISOLATE_WORKERS,
        NODE_ACTION_USER_TIMEOUT,
        POSTGRES_MAX_CONNECTIONS,
        SEARCH_INDEX_BUILD_CONCURRENCY,
        SEARCH_INDEX_COMPACTION_CONCURRENCY,
        SEARCH_INDEX_WRITER_QUEUE_SIZE,
        SEARCH_INDEX_WRITER_THREADS,
        TABLE_SUMMARY_SNAPSHOT_CONCURRENCY,
        UDF_CACHE_MAX_SIZE,
        VERTICAL_SCALING_CPU_COUNT,
        VERTICAL_SCALING_ENABLED,
        VERTICAL_SCALING_RESERVED_CPU_COUNT,
    },
    persistence::Persistence,
    runtime::{
        new_rate_limiter,
        Runtime,
    },
    shutdown::ShutdownSignal,
    types::{
        ConvexOrigin,
        ConvexSite,
        DeploymentClass,
        DeploymentMetadata,
        TEST_REGION_NAME,
    },
};
use config::LocalConfig;
use database::Database;
use events::usage::NoOpUsageEventLogger;
use exports::interface::InProcessExportProvider;
use file_storage::{
    FileStorage,
    TransactionalFileStorage,
};
use function_runner::{
    in_process_function_runner::InProcessFunctionRunner,
    server::DeploymentStorage,
    FunctionRunner,
};
use governor::Quota;
use http_client::CachedHttpClient;
use indexing::index_cache::IndexCache;
use model::{
    initialize_application_system_tables,
    virtual_system_mapping,
};
use node_executor::{
    local::{
        LocalNodeExecutor,
        LOCAL_NODE_EXECUTOR_POOL_SIZE,
    },
    remote::RemoteNodeExecutor,
    NodeActions,
    NodeExecutor,
};
use runtime::prod::ProdRuntime;
use search::{
    searcher::{
        InProcessSearcher,
        MAX_CONCURRENT_SEGMENT_COMPACTIONS,
        MAX_CONCURRENT_SEGMENT_FETCHES,
        MAX_CONCURRENT_TEXT_SEARCHES,
        MAX_CONCURRENT_VECTOR_SEARCHES,
        MAX_CONCURRENT_VECTOR_SEGMENT_PREFETCHES,
        SEARCH_GENERAL_POOL_MAX_CONCURRENCY,
        SEARCH_GENERAL_POOL_QUEUE_SIZE,
    },
    Searcher,
    SegmentTermMetadataFetcher,
};
use serde::Serialize;

pub mod admin;
mod app_metrics;
mod args_structs;
pub mod authentication;
pub mod beacon;
pub mod canonical_urls;
pub mod config;
pub mod custom_headers;
pub mod dashboard;
pub mod deploy_config;
pub mod deploy_config2;
pub mod deployment_audit_log;
pub mod deployment_info;
pub mod deployment_state;
pub mod environment_variables;
pub mod http_actions;
pub mod log_sinks;
pub mod logs;
pub mod node_action_callbacks;
pub mod parse;
pub mod proxy;
pub mod public_api;
pub mod router;
pub mod scheduling;
pub mod schema;
pub mod snapshot_export;
pub mod snapshot_import;
pub mod storage;
pub mod streaming_export;
pub mod streaming_import;
pub mod subs;
pub mod usage_limits;

#[derive(Clone)]
pub struct LocalAppState {
    // Origin for the server (e.g. http://127.0.0.1:3210, https://demo.convex.cloud)
    pub origin: ConvexOrigin,
    // Origin for the corresponding convex.site (where we serve HTTP) (e.g. http://127.0.0.1:8001, https://crazy-giraffe-123.convex.site)
    pub site_origin: ConvexSite,
    // Name of the instance. (e.g. crazy-giraffe-123)
    pub instance_name: String,
    pub application: Application<ProdRuntime>,
    pub zombify_rx: async_broadcast::Receiver<()>,
}

impl LocalAppState {
    pub async fn shutdown(self) -> anyhow::Result<()> {
        self.application.shutdown().await?;

        Ok(())
    }
}

// Contains state needed to serve most http routes. Similar to LocalAppState,
// but uses ApplicationApi instead of Application, which allows it to be used
// in both Backend and Usher.
#[derive(Clone)]
pub struct RouterState {
    pub api: Arc<dyn ApplicationApi>,
    pub runtime: ProdRuntime,
}

#[derive(Serialize)]
pub struct EmptyResponse {}

pub async fn make_app(
    runtime: ProdRuntime,
    config: LocalConfig,
    persistence: Arc<dyn Persistence>,
    zombify_rx: async_broadcast::Receiver<()>,
    preempt_tx: ShutdownSignal,
) -> anyhow::Result<LocalAppState> {
    tracing::info!(
        vertical_scaling_enabled = *VERTICAL_SCALING_ENABLED,
        cpu_count = *VERTICAL_SCALING_CPU_COUNT,
        reserved_cpu_count = *VERTICAL_SCALING_RESERVED_CPU_COUNT,
        active_v8_cpu_limit = *FUNRUN_ISOLATE_ACTIVE_THREADS,
        transaction_isolate_workers = *MAX_TRANSACTION_ISOLATE_WORKERS,
        action_isolate_workers = *MAX_ACTION_ISOLATE_WORKERS,
        max_concurrent_queries = *APPLICATION_MAX_CONCURRENT_QUERIES,
        max_concurrent_mutations = *APPLICATION_MAX_CONCURRENT_MUTATIONS,
        max_concurrent_v8_actions = *APPLICATION_MAX_CONCURRENT_V8_ACTIONS,
        max_concurrent_node_actions = *APPLICATION_MAX_CONCURRENT_NODE_ACTIONS,
        max_concurrent_persistence_writes = *COMMITTER_MAX_CONCURRENT_PERSISTENCE_WRITES,
        postgres_max_connections = *POSTGRES_MAX_CONNECTIONS,
        local_node_executor_processes = *LOCAL_NODE_EXECUTOR_POOL_SIZE,
        async_join_concurrency = *ASYNC_JOIN_CONCURRENCY,
        index_range_batch_concurrency = *INDEX_RANGE_BATCH_CONCURRENCY,
        in_memory_index_load_concurrency = *IN_MEMORY_INDEX_LOAD_CONCURRENCY,
        table_summary_snapshot_concurrency = *TABLE_SUMMARY_SNAPSHOT_CONCURRENCY,
        search_index_build_concurrency = *SEARCH_INDEX_BUILD_CONCURRENCY,
        search_index_compaction_concurrency = *SEARCH_INDEX_COMPACTION_CONCURRENCY,
        search_global_compaction_concurrency = *MAX_CONCURRENT_SEGMENT_COMPACTIONS,
        search_segment_fetch_concurrency = *MAX_CONCURRENT_SEGMENT_FETCHES,
        search_vector_concurrency = *MAX_CONCURRENT_VECTOR_SEARCHES,
        search_text_concurrency = *MAX_CONCURRENT_TEXT_SEARCHES,
        search_vector_prefetch_concurrency = *MAX_CONCURRENT_VECTOR_SEGMENT_PREFETCHES,
        search_general_pool_concurrency = *SEARCH_GENERAL_POOL_MAX_CONCURRENCY,
        search_general_pool_queue_size = *SEARCH_GENERAL_POOL_QUEUE_SIZE,
        search_index_writer_threads = *SEARCH_INDEX_WRITER_THREADS,
        search_index_writer_queue_size = *SEARCH_INDEX_WRITER_QUEUE_SIZE,
        "Resolved single-host execution capacity"
    );
    let key_broker = config.key_broker()?;
    let in_process_searcher = Arc::new(InProcessSearcher::new(runtime.clone())?);
    let searcher: Arc<dyn Searcher> = in_process_searcher.clone();
    // TODO(CX-6572) Separate `SegmentMetadataFetcher` from `SearcherImpl`
    let segment_metadata_fetcher: Arc<dyn SegmentTermMetadataFetcher> = in_process_searcher;
    let (deleted_tablet_sender, deleted_tablet_receiver) = tokio::sync::mpsc::channel(100);
    let usage_event_logger = Arc::new(NoOpUsageEventLogger);
    let database = Database::load(
        persistence.clone(),
        runtime.clone(),
        searcher.clone(),
        preempt_tx.clone(),
        virtual_system_mapping().clone(),
        IndexCache::new(*INDEX_CACHE_SIZE).new_handle(),
        Arc::new(new_rate_limiter(
            runtime.clone(),
            Quota::per_second(*DOCUMENT_RETENTION_RATE_LIMIT),
        )),
        deleted_tablet_sender,
    )
    .await?;
    initialize_application_system_tables(&database).await?;
    let application_storage = Application::initialize_storage(
        runtime.clone(),
        &database,
        config.storage_tag_initializer(),
        config.name(),
    )
    .await?;

    let file_storage = FileStorage {
        transactional_file_storage: TransactionalFileStorage::new(
            runtime.clone(),
            application_storage.files_storage.clone(),
            config.convex_origin_url()?,
        ),
        database: database.clone(),
    };

    let deployment = DeploymentMetadata {
        name: config.name(),
        region: None,
        class: DeploymentClass::S16,
    };
    let node_process_timeout = *NODE_ACTION_USER_TIMEOUT + Duration::from_secs(5);
    let node_executor: Arc<dyn NodeExecutor> =
        match RemoteNodeExecutor::from_env(node_process_timeout)? {
            Some(remote_executor) => Arc::new(remote_executor),
            None => Arc::new(LocalNodeExecutor::new(node_process_timeout).await?),
        };
    let node_actions = NodeActions::new(
        node_executor,
        config.convex_origin_url()?,
        *NODE_ACTION_USER_TIMEOUT,
        runtime.clone(),
        deployment.clone(),
    );

    #[cfg(not(debug_assertions))]
    if config.convex_http_proxy.is_none() {
        tracing::warn!(
            "Running without a proxy in release mode -- UDF `fetch` requests are unrestricted!"
        );
    }
    let fetch_client = Arc::new(ProxiedFetchClient::new(
        config.convex_http_proxy.clone(),
        config.name(),
        reqwest::redirect::Policy::none(),
    ));
    let oidc_http_client = CachedHttpClient::new(
        config.convex_http_proxy.clone(),
        config.name(),
        reqwest::redirect::Policy::default(),
    );
    let function_runner: Arc<dyn FunctionRunner<ProdRuntime>> =
        Arc::new(InProcessFunctionRunner::new(
            deployment,
            key_broker.function_runner_keybroker(),
            config.convex_origin_url()?,
            runtime.clone(),
            persistence.reader(),
            DeploymentStorage {
                files_storage: application_storage.files_storage.clone(),
                modules_storage: application_storage.modules_storage.clone(),
            },
            database.clone(),
            fetch_client.clone(),
        )?);

    let application = Application::new(
        runtime.clone(),
        database.clone(),
        file_storage.clone(),
        application_storage,
        usage_event_logger,
        Arc::new(NoopUsageLimitNotifier),
        key_broker.clone(),
        DeploymentMetadata {
            name: config.name(),
            region: Some(TEST_REGION_NAME.clone()),
            class: DeploymentClass::S16,
        },
        function_runner,
        config.convex_origin_url()?,
        config.convex_site_url()?,
        searcher.clone(),
        segment_metadata_fetcher,
        persistence,
        node_actions,
        Arc::new(RedactLogsToClient::new(config.redact_logs_to_client)),
        Arc::new(ApplicationAuth::new(
            key_broker.clone(),
            Arc::new(NullAccessTokenAuth),
            runtime.clone(),
        )),
        QueryCache::new(*UDF_CACHE_MAX_SIZE),
        fetch_client,
        config.local_log_sink.clone(),
        preempt_tx.clone(),
        Arc::new(InProcessExportProvider),
        deleted_tablet_receiver,
        oidc_http_client,
    )
    .await?;

    let origin = config.convex_origin_url()?;
    let instance_name = config.name();

    if !config.disable_beacon {
        let beacon_future = beacon::start_beacon(
            runtime.clone(),
            database.clone(),
            config.beacon_tag.clone(),
            config.beacon_fields.clone(),
        );
        runtime.spawn_background("beacon_worker", beacon_future);
    }

    let app_state = LocalAppState {
        origin,
        site_origin: config.convex_site_url()?,
        instance_name,
        application,
        zombify_rx,
    };

    Ok(app_state)
}

#[derive(Clone)]
pub struct HttpActionRouteMapper;

impl RouteMapper for HttpActionRouteMapper {
    fn map_route(&self, route: String) -> String {
        // Backend can receive arbitrary HTTP requests, so group all of these
        // under one tag.
        if route.starts_with("/http/") {
            "/http/:user_http_action".into()
        } else {
            route
        }
    }
}
