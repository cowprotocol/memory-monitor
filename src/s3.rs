use {
    aws_sdk_s3::{primitives::ByteStream, Client},
    std::path::Path,
    tracing::info,
};

pub fn s3_console_url(bucket: &str, key: &str) -> String {
    format!(
        "https://s3.console.aws.amazon.com/s3/object/{}?prefix={}",
        bucket, key
    )
}

/// Upload a local file to S3. Retries are handled by the SDK's `RetryConfig`.
pub async fn upload_to_s3(
    client: &Client,
    local_file: &Path,
    bucket: &str,
    key: &str,
) -> Result<(), String> {
    let s3_url = format!("s3://{}/{}", bucket, key);
    info!(s3_url, "Uploading to S3...");

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

    let console_url = s3_console_url(bucket, key);
    info!(s3_url, console_url, "Successfully uploaded to S3");
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
