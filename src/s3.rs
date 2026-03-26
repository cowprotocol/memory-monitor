use std::path::Path;

use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use tracing::{error, info};

const MAX_ATTEMPTS: u32 = 3;
const RETRY_DELAY_SECS: u64 = 5;

pub fn s3_console_url(bucket: &str, key: &str) -> String {
    format!(
        "https://s3.console.aws.amazon.com/s3/object/{}?prefix={}",
        bucket, key
    )
}

/// Upload a local file to S3. Retries up to 3 times with 5s delay.
pub async fn upload_to_s3(
    client: &Client,
    local_file: &Path,
    bucket: &str,
    key: &str,
) -> Result<(), String> {
    let s3_url = format!("s3://{}/{}", bucket, key);
    info!(s3_url, "Uploading to S3...");

    for attempt in 1..=MAX_ATTEMPTS {
        info!(attempt, MAX_ATTEMPTS, "Upload attempt...");

        match try_upload(client, local_file, bucket, key).await {
            Ok(()) => {
                let console_url = s3_console_url(bucket, key);
                info!(s3_url, console_url, "Successfully uploaded to S3");
                return Ok(());
            }
            Err(err) => {
                error!(attempt, ?err, "Upload attempt failed");
                if attempt < MAX_ATTEMPTS {
                    info!(delay_secs = RETRY_DELAY_SECS, "Retrying...");
                    tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
                }
            }
        }
    }

    Err(format!(
        "ERROR: Failed to upload to S3 after {} attempts",
        MAX_ATTEMPTS
    ))
}

async fn try_upload(
    client: &Client,
    local_file: &Path,
    bucket: &str,
    key: &str,
) -> Result<(), String> {
    let body = ByteStream::from_path(local_file)
        .await
        .map_err(|e| format!("Failed to read file {:?}: {}", local_file, e))?;

    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("S3 PutObject failed: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_s3_console_url() {
        let url = s3_console_url("my-bucket", "path/to/file.pprof");
        assert_eq!(
            url,
            "https://s3.console.aws.amazon.com/s3/object/my-bucket?prefix=path/to/file.pprof"
        );
    }
}
