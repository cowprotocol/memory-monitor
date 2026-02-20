mod config;
mod detection;
mod heap_dump;
mod history;
mod process;
mod s3;
mod slack;

use config::Config;
use detection::{DetectionMode, Detector};
use history::History;
use tracing::{error, info, warn};

fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / 1024 / 1024
}

struct Monitor {
    config: Config,
    s3_client: aws_sdk_s3::Client,
}

impl Monitor {
    /// Create a heap dump, upload it to S3, and optionally send a Slack notification.
    async fn create_and_upload_dump(
        &self,
        current_memory: u64,
        baseline_memory: u64,
        mode: &str,
    ) -> Result<(), String> {
        let timestamp = chrono::Utc::now().format("%Y-%m-%d-%H-%M-%S");
        let dump_file = format!("/tmp/{}-{}-{}.pprof", self.config.pod_name, timestamp, mode);
        let filename = format!("{}-{}-{}.pprof", self.config.pod_name, timestamp, mode);
        let s3_key = format!("{}{}", self.config.s3_path_prefix, filename);

        heap_dump::create_heap_dump(&self.config.binary_name, &dump_file).await?;

        let upload_result =
            s3::upload_to_s3(&self.s3_client, &dump_file, &self.config.s3_bucket, &s3_key).await;

        // Send Slack notification if upload succeeded and not a baseline dump
        if upload_result.is_ok() && mode != "baseline" {
            let notification = slack::SlackNotification {
                token: self.config.slack_api_token.as_deref(),
                environment: self.config.environment.as_deref(),
                network: self.config.network.as_deref(),
                pod_name: &self.config.pod_name,
                binary_name: &self.config.binary_name,
                current_memory,
                baseline_memory,
                bucket: &self.config.s3_bucket,
                s3_key: &s3_key,
                mode,
            };
            if let Err(e) = slack::send_slack_notification(&notification).await {
                error!(error = %e, "Failed to send Slack notification");
            }
        }

        // Clean up local dump file regardless of upload result
        heap_dump::cleanup_dump_file(&dump_file).await;

        upload_result
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!("{}", e);
            std::process::exit(1);
        }
    };

    info!("{}", config);

    let check_interval = std::time::Duration::from_secs(config.check_interval);
    let initial_delay = std::time::Duration::from_secs(config.initial_delay);
    let history_window_size = config.history_window_size;
    let spike_cooldown_secs = config.spike_cooldown();

    // Initialize S3 client (uses pod IAM role automatically)
    let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    let monitor = Monitor { config, s3_client };

    // Wait for initial delay before starting monitoring
    info!(
        delay_secs = initial_delay.as_secs(),
        "Sleeping before starting monitoring..."
    );
    tokio::time::sleep(initial_delay).await;
    info!("Initial delay complete");

    // Create baseline dump
    info!("Creating baseline dump before starting history collection...");
    let initial_usage = match process::get_process_memory(&monitor.config.binary_name) {
        Some(usage) => usage,
        None => {
            error!(
                binary_name = monitor.config.binary_name,
                "Process not found for baseline dump"
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = monitor
        .create_and_upload_dump(initial_usage, 0, "baseline")
        .await
    {
        error!(error = %e, "Failed to create/upload baseline dump");
        std::process::exit(1);
    }
    info!("Baseline dump uploaded successfully");

    // Sleep to allow memory to settle after dump
    info!("Sleeping for 60s to allow memory to settle after baseline dump...");
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;

    let mut history = History::new(history_window_size);
    let mut detector = Detector::new(
        monitor.config.dump_cooldown,
        spike_cooldown_secs,
        monitor.config.spike_multiplier,
        monitor.config.memory_change_threshold,
    );

    loop {
        let usage = match process::get_process_memory(&monitor.config.binary_name) {
            Some(u) => u,
            None => {
                warn!(
                    binary_name = monitor.config.binary_name,
                    "Process not found or unable to read process memory. Will retry..."
                );
                tokio::time::sleep(check_interval).await;
                continue;
            }
        };

        history.push(usage);

        if !history.is_full() {
            info!(
                samples = history.len(),
                window = history_window_size,
                current_mb = bytes_to_mb(usage),
                "Building history..."
            );
            tokio::time::sleep(check_interval).await;
            continue;
        }

        let current_p50 = history.percentile(50);
        let current_p95 = history.percentile(95);

        // Establish baseline on first full window
        if detector.baseline_p50 == 0 {
            detector.baseline_p50 = current_p50;
            info!(
                baseline_p50_mb = bytes_to_mb(detector.baseline_p50),
                current_p95_mb = bytes_to_mb(current_p95),
                "Baseline established"
            );
            tokio::time::sleep(check_interval).await;
            continue;
        }

        // Check for anomalies
        if let Some(detection) = detector.check(usage, current_p50, current_p95) {
            match detection.mode {
                DetectionMode::Spike => {
                    let threshold = current_p95 * monitor.config.spike_multiplier;
                    info!(
                        usage_mb = bytes_to_mb(usage),
                        multiplier = monitor.config.spike_multiplier,
                        p95_mb = bytes_to_mb(current_p95),
                        threshold_mb = bytes_to_mb(threshold),
                        "SPIKE DETECTED"
                    );
                }
                DetectionMode::SlowLeak => {
                    let threshold = detector.baseline_p50 + monitor.config.memory_change_threshold;
                    info!(
                        p50_mb = bytes_to_mb(current_p50),
                        baseline_p50_mb = bytes_to_mb(detector.baseline_p50),
                        threshold_mb = bytes_to_mb(monitor.config.memory_change_threshold),
                        limit_mb = bytes_to_mb(threshold),
                        "SLOW LEAK DETECTED"
                    );
                }
            }

            if detector.cooldown_passed(detection.mode) {
                match monitor
                    .create_and_upload_dump(
                        usage,
                        detection.baseline_for_notification,
                        detection.mode.as_str(),
                    )
                    .await
                {
                    Ok(()) => {
                        detector.record_dump(detection.mode, current_p50);
                        info!(
                            baseline_p50_mb = bytes_to_mb(detector.baseline_p50),
                            current_p95_mb = bytes_to_mb(current_p95),
                            "Baseline updated"
                        );
                    }
                    Err(e) => {
                        error!(
                            mode = detection.mode.as_str(),
                            error = %e,
                            "Failed to create or upload heap dump"
                        );
                    }
                }
            }
        }

        tokio::time::sleep(check_interval).await;
    }
}
