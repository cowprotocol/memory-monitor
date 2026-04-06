use {
    bytesize::ByteSize,
    std::{env, fmt, time::Duration},
};

pub struct Config {
    /// Name of the target process to monitor (matched against `/proc/*/comm`).
    pub binary_name: String,
    /// How often to sample process memory.
    pub check_interval: Duration,
    /// Slow-leak threshold: a dump is triggered when current P50 exceeds
    /// baseline P50 by more than this amount (in bytes).
    pub memory_change_threshold: u64,
    /// Time to wait after startup before capturing the baseline heap dump.
    pub initial_delay: Duration,
    /// Minimum time between consecutive slow-leak heap dumps.
    pub dump_cooldown: Duration,
    /// S3 bucket for uploading heap dumps.
    pub s3_bucket: String,
    /// Key prefix inside the S3 bucket (e.g. `memory-dumps/`).
    pub s3_path_prefix: String,
    /// Kubernetes pod name, used in dump filenames and Slack messages.
    pub pod_name: String,
    /// Number of memory samples kept in the sliding window (default 60).
    pub history_window_size: usize,
    /// Spike detection multiplier: a spike is detected when instantaneous
    /// usage exceeds `P95 * spike_multiplier` (default 3).
    pub spike_multiplier: u64,
    /// Slack Bot OAuth token. If absent, Slack notifications are skipped.
    pub slack_api_token: Option<String>,
    /// Deployment environment (e.g. `prod`, `staging`). Used for Slack
    /// channel routing and alert formatting.
    pub environment: Option<String>,
    /// Blockchain network name (e.g. `mainnet`). Included in Slack alerts.
    pub network: Option<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("binary_name", &self.binary_name)
            .field("check_interval", &self.check_interval)
            .field("memory_change_threshold", &self.memory_change_threshold)
            .field("initial_delay", &self.initial_delay)
            .field("dump_cooldown", &self.dump_cooldown)
            .field("s3_bucket", &self.s3_bucket)
            .field("s3_path_prefix", &self.s3_path_prefix)
            .field("pod_name", &self.pod_name)
            .field("history_window_size", &self.history_window_size)
            .field("spike_multiplier", &self.spike_multiplier)
            .field(
                "slack_api_token",
                &self.slack_api_token.as_ref().map(|_| "REDACTED"),
            )
            .field("environment", &self.environment)
            .field("network", &self.network)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let binary_name = required_env("BINARY_NAME")?;
        let check_interval = required_env_duration("CHECK_INTERVAL")?;
        let memory_change_threshold =
            required_env_parsed::<ByteSize>("MEMORY_CHANGE_THRESHOLD")?.as_u64();
        let initial_delay = required_env_duration("INITIAL_DELAY")?;
        let dump_cooldown = required_env_duration("DUMP_COOLDOWN")?;
        let s3_bucket = required_env("S3_BUCKET")?;
        let s3_path_prefix = required_env("S3_PATH_PREFIX")?;
        let pod_name = required_env("POD_NAME")?;
        let history_window_size = optional_env_parsed::<usize>("HISTORY_WINDOW_SIZE", 60)?;
        let spike_multiplier = optional_env_parsed::<u64>("SPIKE_MULTIPLIER", 3)?;
        let slack_api_token = optional_env("SLACK_API_TOKEN");
        let environment = optional_env("ENVIRONMENT");
        let network = optional_env("NETWORK");

        Ok(Self {
            binary_name,
            check_interval,
            memory_change_threshold,
            initial_delay,
            dump_cooldown,
            s3_bucket,
            s3_path_prefix,
            pod_name,
            history_window_size,
            spike_multiplier,
            slack_api_token,
            environment,
            network,
        })
    }

    pub fn spike_cooldown(&self) -> Duration {
        self.check_interval * self.history_window_size as u32
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Memory monitor started for process: {}; Monitoring process RssAnon (anonymous \
             memory: heap/stack) via /proc; Check interval: {}; Detection mode: Dual (spike + \
             slow leak); Spike threshold: {}x P95; Memory change threshold: P50 + {}; History \
             window: {} samples; Initial delay before first dump: {}; Dump cooldown: {}; Spike \
             cooldown: {} (history window refresh); S3 destination: s3://{}/{}",
            self.binary_name,
            humantime::format_duration(self.check_interval),
            self.spike_multiplier,
            ByteSize(self.memory_change_threshold),
            self.history_window_size,
            humantime::format_duration(self.initial_delay),
            humantime::format_duration(self.dump_cooldown),
            humantime::format_duration(self.spike_cooldown()),
            self.s3_bucket,
            self.s3_path_prefix,
        )
    }
}

fn required_env(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("ERROR: {} is required", key))
}

fn required_env_parsed<T: std::str::FromStr>(key: &str) -> Result<T, String>
where
    T::Err: fmt::Display,
{
    let val = required_env(key)?;
    val.parse::<T>()
        .map_err(|e| format!("ERROR: {} has invalid value '{}': {}", key, val, e))
}

fn required_env_duration(key: &str) -> Result<Duration, String> {
    let val = required_env(key)?;
    humantime::parse_duration(&val)
        .map_err(|e| format!("ERROR: {} has invalid duration '{}': {}", key, val, e))
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

fn optional_env_parsed<T: std::str::FromStr>(key: &str, default: T) -> Result<T, String>
where
    T::Err: fmt::Display,
{
    match env::var(key) {
        Ok(val) if !val.is_empty() => val
            .parse::<T>()
            .map_err(|e| format!("ERROR: {} has invalid value '{}': {}", key, val, e)),
        _ => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use {super::*, std::sync::Mutex};

    // Environment variable tests need serialization since env vars are
    // process-global.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn set_required_env_vars() {
        env::set_var("BINARY_NAME", "driver");
        env::set_var("CHECK_INTERVAL", "10s");
        env::set_var("MEMORY_CHANGE_THRESHOLD", "200MB");
        env::set_var("INITIAL_DELAY", "1h");
        env::set_var("DUMP_COOLDOWN", "1m");
        env::set_var("S3_BUCKET", "my-bucket");
        env::set_var("S3_PATH_PREFIX", "memory-dumps/");
        env::set_var("POD_NAME", "test-pod-abc123");
    }

    fn clear_all_env_vars() {
        for key in [
            "BINARY_NAME",
            "CHECK_INTERVAL",
            "MEMORY_CHANGE_THRESHOLD",
            "INITIAL_DELAY",
            "DUMP_COOLDOWN",
            "S3_BUCKET",
            "S3_PATH_PREFIX",
            "POD_NAME",
            "HISTORY_WINDOW_SIZE",
            "SPIKE_MULTIPLIER",
            "SLACK_API_TOKEN",
            "ENVIRONMENT",
            "NETWORK",
        ] {
            env::remove_var(key);
        }
    }

    #[test]
    fn test_from_env_all_required() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        set_required_env_vars();

        let config = Config::from_env().unwrap();
        assert_eq!(config.binary_name, "driver");
        assert_eq!(config.check_interval, Duration::from_secs(10));
        assert_eq!(config.memory_change_threshold, 200_000_000);
        assert_eq!(config.initial_delay, Duration::from_secs(3600));
        assert_eq!(config.dump_cooldown, Duration::from_secs(60));
        assert_eq!(config.s3_bucket, "my-bucket");
        assert_eq!(config.s3_path_prefix, "memory-dumps/");
        assert_eq!(config.pod_name, "test-pod-abc123");
        assert_eq!(config.history_window_size, 60);
        assert_eq!(config.spike_multiplier, 3);
        assert!(config.slack_api_token.is_none());
        assert!(config.environment.is_none());
        assert!(config.network.is_none());

        clear_all_env_vars();
    }

    #[test]
    fn test_from_env_with_optionals() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        set_required_env_vars();
        env::set_var("HISTORY_WINDOW_SIZE", "120");
        env::set_var("SPIKE_MULTIPLIER", "5");
        env::set_var("SLACK_API_TOKEN", "xoxb-test-token");
        env::set_var("ENVIRONMENT", "prod");
        env::set_var("NETWORK", "mainnet");

        let config = Config::from_env().unwrap();
        assert_eq!(config.history_window_size, 120);
        assert_eq!(config.spike_multiplier, 5);
        assert_eq!(config.slack_api_token.as_deref(), Some("xoxb-test-token"));
        assert_eq!(config.environment.as_deref(), Some("prod"));
        assert_eq!(config.network.as_deref(), Some("mainnet"));

        clear_all_env_vars();
    }

    #[test]
    fn test_missing_required_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let err = Config::from_env().unwrap_err();
        assert!(err.contains("BINARY_NAME is required"), "got: {}", err);

        clear_all_env_vars();
    }

    #[test]
    fn test_invalid_duration_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        set_required_env_vars();
        env::set_var("CHECK_INTERVAL", "not_a_duration");

        let err = Config::from_env().unwrap_err();
        assert!(
            err.contains("CHECK_INTERVAL") && err.contains("invalid duration"),
            "got: {}",
            err
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_spike_cooldown() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();
        set_required_env_vars();

        let config = Config::from_env().unwrap();
        // 60 * 10s = 600s
        assert_eq!(config.spike_cooldown(), Duration::from_secs(600));

        clear_all_env_vars();
    }
}
