use crate::detection::DumpMode;
use serde::Serialize;
use tracing::{error, info, warn};

#[derive(Serialize)]
struct SlackMessage {
    channel: String,
    attachments: Vec<SlackAttachment>,
}

#[derive(Serialize)]
struct SlackAttachment {
    color: String,
    text: String,
}

fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / 1024 / 1024
}

fn select_channel(environment: &str) -> &'static str {
    match environment {
        "prod" => "alerts-prod",
        "staging" | "shadow" => "alerts-barn",
        _ => {
            warn!(
                environment,
                "Unknown environment, defaulting to alerts-temp"
            );
            "alerts-temp"
        }
    }
}

fn s3_console_url(bucket: &str, key: &str) -> String {
    format!(
        "https://s3.console.aws.amazon.com/s3/object/{}?prefix={}",
        bucket, key
    )
}

fn mode_display(mode: DumpMode) -> &'static str {
    match mode {
        DumpMode::Spike => "\u{1f6a8}Spike",
        DumpMode::SlowLeak => "\u{1f40c}Slow Leak",
        DumpMode::Baseline => "Baseline",
    }
}

/// Parameters for sending a Slack notification.
pub struct SlackNotification<'a> {
    pub token: Option<&'a str>,
    pub environment: Option<&'a str>,
    pub network: Option<&'a str>,
    pub pod_name: &'a str,
    pub binary_name: &'a str,
    pub current_memory: u64,
    pub baseline_memory: u64,
    pub bucket: &'a str,
    pub s3_key: &'a str,
    pub mode: DumpMode,
}

/// Send a Slack notification about a memory anomaly.
/// Silently returns Ok if no token is configured.
pub async fn send_slack_notification(params: &SlackNotification<'_>) -> Result<(), String> {
    let token = match params.token {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(()),
    };

    let (environment, network) = match (params.environment, params.network) {
        (Some(e), Some(n)) => (e, n),
        _ => {
            warn!("SLACK_API_TOKEN set but missing ENVIRONMENT or NETWORK env vars");
            return Err(
                "SLACK_API_TOKEN set but missing ENVIRONMENT or NETWORK env vars".to_string(),
            );
        }
    };

    let channel = select_channel(environment);
    let console_url = s3_console_url(params.bucket, params.s3_key);
    let current_mb = bytes_to_mb(params.current_memory);
    let baseline_mb = bytes_to_mb(params.baseline_memory);
    let increase_mb = current_mb.saturating_sub(baseline_mb);
    let mode_str = mode_display(params.mode);

    let message = format!(
        "*Memory increase detected in {}-{}-{}*\n\
         Pod: `{}`\n\
         Detection: *{}*\n\
         Memory increased by *{} MB* ({} MB \u{2192} {} MB)\n\
         Heap dump uploaded: {}",
        network,
        params.binary_name,
        environment,
        params.pod_name,
        mode_str,
        increase_mb,
        baseline_mb,
        current_mb,
        console_url,
    );

    let payload = SlackMessage {
        channel: channel.to_string(),
        attachments: vec![SlackAttachment {
            color: "warning".to_string(),
            text: message,
        }],
    };

    info!(channel, "Sending Slack notification...");

    let client = reqwest::Client::new();
    let resp = client
        .post("https://slack.com/api/chat.postMessage")
        .header("Authorization", format!("Bearer {}", token))
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Failed to send Slack notification: {}", e))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Slack response: {}", e))?;

    if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        info!("Slack notification sent successfully");
        Ok(())
    } else {
        let err = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        error!(?err, "Slack API error");
        Err(format!("Slack API error: {}", err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_channel() {
        assert_eq!(select_channel("prod"), "alerts-prod");
        assert_eq!(select_channel("staging"), "alerts-barn");
        assert_eq!(select_channel("shadow"), "alerts-barn");
        assert_eq!(select_channel("dev"), "alerts-temp");
        assert_eq!(select_channel(""), "alerts-temp");
    }

    #[test]
    fn test_s3_console_url() {
        let url = s3_console_url("my-bucket", "path/to/file.pprof");
        assert_eq!(
            url,
            "https://s3.console.aws.amazon.com/s3/object/my-bucket?prefix=path/to/file.pprof"
        );
    }

    #[test]
    fn test_mode_display() {
        assert_eq!(mode_display(DumpMode::Spike), "\u{1f6a8}Spike");
        assert_eq!(mode_display(DumpMode::SlowLeak), "\u{1f40c}Slow Leak");
        assert_eq!(mode_display(DumpMode::Baseline), "Baseline");
    }

    #[test]
    fn test_bytes_to_mb() {
        assert_eq!(bytes_to_mb(1024 * 1024), 1);
        assert_eq!(bytes_to_mb(500 * 1024 * 1024), 500);
        assert_eq!(bytes_to_mb(0), 0);
    }
}
