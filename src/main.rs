mod config;
mod detection;
mod heap_dump;
mod history;
mod process;
mod s3;
mod slack;

use std::path::PathBuf;

use bytesize::ByteSize;
use config::Config;
use detection::{DetectionMode, Detector, DumpMode};
use history::History;
use tracing::{error, info, warn};

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
        mode: DumpMode,
    ) -> Result<(), String> {
        let timestamp = chrono::Utc::now().format("%Y-%m-%d-%H-%M-%S");
        let filename = format!("{}-{}-{}.pprof", self.config.pod_name, timestamp, mode);
        let dump_file = PathBuf::from(format!("/tmp/{}", filename));
        let s3_key = format!("{}{}", self.config.s3_path_prefix, filename);

        heap_dump::create_heap_dump(&self.config.binary_name, &dump_file).await?;

        let upload_result =
            s3::upload_to_s3(&self.s3_client, &dump_file, &self.config.s3_bucket, &s3_key).await;

        // Send Slack notification if upload succeeded and not a baseline dump
        if upload_result.is_ok() && !mode.is_baseline() {
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
            if let Err(err) = slack::send_slack_notification(&notification).await {
                error!(?err, "Failed to send Slack notification");
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
        Ok(config) => config,
        Err(err) => {
            error!(?err, "error loading config");
            std::process::exit(1);
        }
    };

    info!(?config, "loaded config");

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

    if let Err(err) = monitor
        .create_and_upload_dump(initial_usage, 0, DumpMode::Baseline)
        .await
    {
        error!(?err, "Failed to create/upload baseline dump");
        std::process::exit(1);
    }
    info!("Baseline dump uploaded successfully");

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
        let Some(usage) = process::get_process_memory(&monitor.config.binary_name) else {
            warn!(
                binary_name = monitor.config.binary_name,
                "Process not found or unable to read process memory. Will retry..."
            );
            tokio::time::sleep(check_interval).await;
            continue;
        };

        history.push(usage);

        if !history.is_full() {
            info!(
                samples = history.len(),
                window = history_window_size,
                current = %ByteSize(usage),
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
                baseline_p50 = %ByteSize(detector.baseline_p50),
                current_p95 = %ByteSize(current_p95),
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
                        usage = %ByteSize(usage),
                        multiplier = monitor.config.spike_multiplier,
                        p95 = %ByteSize(current_p95),
                        threshold = %ByteSize(threshold),
                        "SPIKE DETECTED"
                    );
                }
                DetectionMode::SlowLeak => {
                    let threshold = detector.baseline_p50 + monitor.config.memory_change_threshold;
                    info!(
                        p50 = %ByteSize(current_p50),
                        baseline_p50 = %ByteSize(detector.baseline_p50),
                        threshold = %ByteSize(monitor.config.memory_change_threshold),
                        limit = %ByteSize(threshold),
                        "SLOW LEAK DETECTED"
                    );
                }
            }

            if detector.cooldown_passed(detection.mode) {
                match monitor
                    .create_and_upload_dump(
                        usage,
                        detection.baseline_for_notification,
                        detection.mode.into(),
                    )
                    .await
                {
                    Ok(()) => {
                        detector.record_dump(detection.mode, current_p50);
                        info!(
                            baseline_p50 = %ByteSize(detector.baseline_p50),
                            current_p95 = %ByteSize(current_p95),
                            "Baseline updated"
                        );
                    }
                    Err(err) => {
                        error!(
                            mode = %DumpMode::from(detection.mode),
                            ?err,
                            "Failed to create or upload heap dump"
                        );
                    }
                }
            }
        }

        tokio::time::sleep(check_interval).await;
    }
}
