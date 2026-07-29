use std::{
    sync::atomic::{
        AtomicU32,
        AtomicU64,
        AtomicUsize,
        Ordering,
    },
    time::{
        Duration,
        SystemTime,
        UNIX_EPOCH,
    },
};

use anyhow::Context;
use async_trait::async_trait;
use common::log_lines::LogLine;
use reqwest::{
    header::HeaderValue,
    Client,
    StatusCode,
    Url,
};
use tokio::sync::mpsc;

use crate::{
    executor::{
        ExecutorRequest,
        InvokeResponse,
        NodeExecutor,
        ARGS_TOO_LARGE_RESPONSE_MESSAGE,
    },
    handle_node_executor_stream,
    local::node_executor_response_stream,
    metrics::log_remote_node_executor_failure,
};

const NODE_EXECUTOR_URLS_ENV: &str = "NODE_EXECUTOR_URLS";
const NODE_EXECUTOR_SHARED_SECRET_ENV: &str = "NODE_EXECUTOR_SHARED_SECRET";
const NODE_EXECUTOR_FAILURE_COOLDOWN_SECONDS_ENV: &str = "NODE_EXECUTOR_FAILURE_COOLDOWN_SECONDS";
const NODE_EXECUTOR_AUTH_HEADER: &str = "x-convex-node-executor-secret";
const MIN_SHARED_SECRET_BYTES: usize = 32;

/// An authenticated pool of stateless Node executor services.
///
/// An executor request may perform arbitrary external side effects. Therefore,
/// transport failures are deliberately returned to the caller rather than
/// retried on another worker.
pub struct RemoteNodeExecutor {
    workers: Vec<RemoteNodeExecutorWorker>,
    next_worker: AtomicUsize,
    client: Client,
    node_process_timeout: Duration,
    failure_cooldown: Duration,
    shared_secret: HeaderValue,
}

struct RemoteNodeExecutorWorker {
    invoke_url: Url,
    in_flight: AtomicUsize,
    consecutive_failures: AtomicU32,
    unhealthy_until_epoch_ms: AtomicU64,
}

struct InFlightGuard<'a> {
    in_flight: &'a AtomicUsize,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

impl RemoteNodeExecutor {
    /// Build a remote executor pool when `NODE_EXECUTOR_URLS` is configured.
    ///
    /// The URL list is comma-separated. A shared secret is mandatory because
    /// the executor endpoint accepts source packages and executes code.
    pub fn from_env(node_process_timeout: Duration) -> anyhow::Result<Option<Self>> {
        let raw_urls = match std::env::var(NODE_EXECUTOR_URLS_ENV) {
            Ok(raw_urls) if !raw_urls.trim().is_empty() => raw_urls,
            Ok(_) | Err(std::env::VarError::NotPresent) => return Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => {
                anyhow::bail!("{NODE_EXECUTOR_URLS_ENV} must contain valid UTF-8")
            },
        };
        let urls = parse_executor_urls(&raw_urls)?;
        let shared_secret = std::env::var(NODE_EXECUTOR_SHARED_SECRET_ENV).with_context(|| {
            format!(
                "{NODE_EXECUTOR_SHARED_SECRET_ENV} is required when {NODE_EXECUTOR_URLS_ENV} is \
                 configured"
            )
        })?;
        anyhow::ensure!(
            shared_secret.len() >= MIN_SHARED_SECRET_BYTES,
            "{NODE_EXECUTOR_SHARED_SECRET_ENV} must be at least {MIN_SHARED_SECRET_BYTES} bytes"
        );
        let shared_secret = HeaderValue::from_str(&shared_secret)
            .with_context(|| format!("{NODE_EXECUTOR_SHARED_SECRET_ENV} is not a valid header"))?;
        let client = Client::builder()
            .pool_max_idle_per_host(urls.len().max(1) * 2)
            .build()
            .context("failed to create remote Node executor HTTP client")?;
        let failure_cooldown = Duration::from_secs(
            cmd_util::env::env_config(NODE_EXECUTOR_FAILURE_COOLDOWN_SECONDS_ENV, 5_u64).max(1),
        );

        tracing::info!(
            worker_count = urls.len(),
            "Configuring authenticated remote Node executor pool"
        );
        Ok(Some(Self::new(
            urls,
            client,
            node_process_timeout,
            failure_cooldown,
            shared_secret,
        )))
    }

    fn new(
        invoke_urls: Vec<Url>,
        client: Client,
        node_process_timeout: Duration,
        failure_cooldown: Duration,
        shared_secret: HeaderValue,
    ) -> Self {
        Self {
            workers: invoke_urls
                .into_iter()
                .map(|invoke_url| RemoteNodeExecutorWorker {
                    invoke_url,
                    in_flight: AtomicUsize::new(0),
                    consecutive_failures: AtomicU32::new(0),
                    unhealthy_until_epoch_ms: AtomicU64::new(0),
                })
                .collect(),
            next_worker: AtomicUsize::new(0),
            client,
            node_process_timeout,
            failure_cooldown,
            shared_secret,
        }
    }

    fn select_worker(
        &self,
    ) -> anyhow::Result<(usize, &RemoteNodeExecutorWorker, InFlightGuard<'_>)> {
        let now_epoch_ms = epoch_millis();
        let start = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let mut selected = None;
        for offset in 0..self.workers.len() {
            let index = (start + offset) % self.workers.len();
            let worker = &self.workers[index];
            if worker.unhealthy_until_epoch_ms.load(Ordering::Relaxed) > now_epoch_ms {
                continue;
            }
            let load = self.workers[index].in_flight.load(Ordering::Relaxed);
            if selected.is_none_or(|(_, selected_load)| load < selected_load) {
                selected = Some((index, load));
            }
        }
        let (selected_index, _) = selected.ok_or_else(|| {
            log_remote_node_executor_failure("all_workers_cooling_down");
            anyhow::anyhow!("all remote Node executor workers are in failure cooldown")
        })?;
        let worker = &self.workers[selected_index];
        worker.in_flight.fetch_add(1, Ordering::Relaxed);
        Ok((
            selected_index,
            worker,
            InFlightGuard {
                in_flight: &worker.in_flight,
            },
        ))
    }

    fn mark_worker_failed(
        &self,
        worker_index: usize,
        worker: &RemoteNodeExecutorWorker,
        failure_type: &'static str,
    ) {
        log_remote_node_executor_failure(failure_type);
        let failures = worker
            .consecutive_failures
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let multiplier = 1_u32 << failures.saturating_sub(1).min(3);
        let cooldown = self.failure_cooldown.saturating_mul(multiplier);
        let unhealthy_until =
            epoch_millis().saturating_add(cooldown.as_millis().try_into().unwrap_or(u64::MAX));
        worker
            .unhealthy_until_epoch_ms
            .store(unhealthy_until, Ordering::Relaxed);
        tracing::warn!(
            worker_index,
            failures,
            cooldown_ms = cooldown.as_millis(),
            "Remote Node executor entered failure cooldown"
        );
    }

    fn mark_worker_healthy(&self, worker: &RemoteNodeExecutorWorker) {
        worker.consecutive_failures.store(0, Ordering::Relaxed);
        worker.unhealthy_until_epoch_ms.store(0, Ordering::Relaxed);
    }
}

#[async_trait]
impl NodeExecutor for RemoteNodeExecutor {
    fn enable(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn invoke(
        &self,
        request: ExecutorRequest,
        log_line_sender: mpsc::UnboundedSender<LogLine>,
    ) -> anyhow::Result<InvokeResponse> {
        let (worker_index, worker, _in_flight_guard) = self.select_worker()?;
        let request_json = serde_json::Value::try_from(request)?;
        let response = match self
            .client
            .post(worker.invoke_url.clone())
            .header(NODE_EXECUTOR_AUTH_HEADER, self.shared_secret.clone())
            .json(&request_json)
            .timeout(self.node_process_timeout)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.mark_worker_failed(worker_index, worker, "transport");
                return Err(error).with_context(|| {
                    format!(
                        "remote Node executor worker {worker_index} request failed; not retrying"
                    )
                });
            },
        };

        if let Err(error) = response.error_for_status_ref() {
            let status = response.status();
            if status == StatusCode::PAYLOAD_TOO_LARGE {
                return Err(anyhow::anyhow!(error.without_url()).context(
                    errors::ErrorMetadata::bad_request(
                        "ArgsTooLarge",
                        ARGS_TOO_LARGE_RESPONSE_MESSAGE,
                    ),
                ));
            }
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                self.mark_worker_failed(worker_index, worker, "authentication");
                anyhow::bail!("remote Node executor worker {worker_index} rejected authentication");
            }
            if status.is_server_error() {
                self.mark_worker_failed(worker_index, worker, "server");
            }
            let body = response.text().await?;
            anyhow::bail!("remote Node executor worker {worker_index} returned {status}: {body}");
        }

        let stream = node_executor_response_stream(self.node_process_timeout, response);
        let result = match handle_node_executor_stream(log_line_sender, Box::pin(stream)).await {
            Ok(result) => result,
            Err(error) => {
                self.mark_worker_failed(worker_index, worker, "response_stream");
                return Err(error);
            },
        };
        self.mark_worker_healthy(worker);
        match result {
            Ok(payload) => Ok(InvokeResponse {
                response: payload,
                aws_request_id: None,
            }),
            Err(error) => Ok(error),
        }
    }

    fn shutdown(&self) {}
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn parse_executor_urls(raw_urls: &str) -> anyhow::Result<Vec<Url>> {
    let mut invoke_urls = Vec::new();
    for raw_url in raw_urls
        .split(',')
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let mut url = Url::parse(raw_url)
            .with_context(|| format!("invalid remote Node executor URL {raw_url:?}"))?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "remote Node executor URL must use http or https: {raw_url:?}"
        );
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none(),
            "remote Node executor URL must not contain credentials"
        );
        anyhow::ensure!(
            url.query().is_none() && url.fragment().is_none(),
            "remote Node executor URL must not contain a query or fragment"
        );
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        invoke_urls.push(url.join("invoke")?);
    }
    anyhow::ensure!(
        !invoke_urls.is_empty(),
        "{NODE_EXECUTOR_URLS_ENV} must contain at least one URL"
    );
    Ok(invoke_urls)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_executor(worker_count: usize) -> RemoteNodeExecutor {
        let urls = (0..worker_count)
            .map(|index| Url::parse(&format!("http://worker-{index}:3002/invoke")).unwrap())
            .collect();
        RemoteNodeExecutor::new(
            urls,
            Client::new(),
            Duration::from_secs(1),
            Duration::from_secs(5),
            HeaderValue::from_static("01234567890123456789012345678901"),
        )
    }

    #[test]
    fn parses_and_normalizes_executor_urls() -> anyhow::Result<()> {
        let urls = parse_executor_urls("http://worker-1:3002, https://worker-2/pool")?;
        assert_eq!(urls[0].as_str(), "http://worker-1:3002/invoke");
        assert_eq!(urls[1].as_str(), "https://worker-2/pool/invoke");
        Ok(())
    }

    #[test]
    fn rejects_unsafe_executor_urls() {
        assert!(parse_executor_urls("file:///tmp/executor").is_err());
        assert!(parse_executor_urls("http://user:password@worker:3002").is_err());
        assert!(parse_executor_urls("http://worker:3002?secret=value").is_err());
    }

    #[test]
    fn selects_least_busy_worker() {
        let executor = test_executor(3);
        executor.workers[0].in_flight.store(2, Ordering::Relaxed);
        executor.workers[1].in_flight.store(1, Ordering::Relaxed);

        let (worker_index, _worker, _guard) = executor.select_worker().unwrap();
        assert_eq!(worker_index, 2);
    }

    #[test]
    fn breaks_idle_ties_round_robin() {
        let executor = test_executor(2);
        let (first, _, first_guard) = executor.select_worker().unwrap();
        drop(first_guard);
        let (second, _, _second_guard) = executor.select_worker().unwrap();
        assert_eq!((first, second), (0, 1));
    }

    #[test]
    fn avoids_workers_during_failure_cooldown() {
        let executor = test_executor(2);
        executor.mark_worker_failed(0, &executor.workers[0], "test");

        let (worker_index, _, _guard) = executor.select_worker().unwrap();
        assert_eq!(worker_index, 1);
    }

    #[test]
    fn fails_fast_when_all_workers_are_cooling_down() {
        let executor = test_executor(2);
        executor.mark_worker_failed(0, &executor.workers[0], "test");
        executor.mark_worker_failed(1, &executor.workers[1], "test");

        assert!(executor.select_worker().is_err());
    }
}
