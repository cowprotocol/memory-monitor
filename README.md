# memory-monitor

Kubernetes sidecar that monitors a target process's memory usage on Linux, detects anomalies (spikes and slow leaks), captures jemalloc heap dumps, and uploads them to S3.

## How it works

The sidecar runs alongside the target process in a shared-PID-namespace pod. It reads the process's anonymous memory (`RssAnon` from `/proc/<pid>/status`) on a configurable interval and maintains a sliding window of samples.

### Detection modes

Once the history window is full, a baseline P50 (median) is established. Each tick, two checks run:

- **Spike**: instantaneous memory > P95 × multiplier (default 3x). Catches sudden allocations.
- **Slow leak**: current P50 > baseline P50 + threshold. Catches gradual memory growth.

Each mode has an independent cooldown timer. On detection, the monitor:

1. Captures a heap dump by sending `dump\n` to a jemalloc profiling Unix socket (`/tmp/heap_dump_<binary>.sock`)
2. Uploads the `.pprof` file to S3
3. Sends a Slack notification (if configured)
4. Updates the baseline P50

A baseline dump is always captured on startup (after the initial delay) for comparison.

### Slack channel routing

| Environment       | Channel       |
|-------------------|---------------|
| `prod`            | alerts-prod   |
| `staging`/`shadow`| alerts-barn   |
| other             | alerts-temp   |

## Configuration

All configuration is via environment variables.

### Required

| Variable                  | Description                                      |
|---------------------------|--------------------------------------------------|
| `BINARY_NAME`             | Name of the target process (matched via `/proc/*/comm`) |
| `CHECK_INTERVAL`          | Seconds between memory checks                    |
| `MEMORY_CHANGE_THRESHOLD` | Bytes above baseline P50 to trigger slow-leak detection |
| `INITIAL_DELAY`           | Seconds to wait before capturing the baseline dump |
| `DUMP_COOLDOWN`           | Minimum seconds between slow-leak dumps          |
| `S3_BUCKET`               | S3 bucket for dump uploads                       |
| `S3_PATH_PREFIX`          | S3 key prefix for dump files                     |
| `POD_NAME`                | Kubernetes pod name (used in filenames and alerts) |

### Optional

| Variable              | Default | Description                                     |
|-----------------------|---------|-------------------------------------------------|
| `HISTORY_WINDOW_SIZE` | `60`    | Number of samples in the sliding window         |
| `SPIKE_MULTIPLIER`    | `3`     | Multiplier of P95 for spike detection           |
| `SLACK_API_TOKEN`     |         | Slack Bot token for notifications               |
| `ENVIRONMENT`         |         | Environment name (prod/staging/shadow) for Slack routing |
| `NETWORK`             |         | Network name included in Slack alerts           |

Spike cooldown is computed automatically as `HISTORY_WINDOW_SIZE × CHECK_INTERVAL` (time for a full window refresh).

## Deployment

The sidecar is deployed as a container alongside the target service. Requirements:

- **Shared PID namespace**: `shareProcessNamespace: true` on the pod spec (so the sidecar can read `/proc` of the target process)
- **Shared `/tmp` volume**: `emptyDir` mounted at `/tmp` in both containers (for the jemalloc Unix socket)
- **IAM role**: Pod must have an IAM role with S3 write permissions (credentials are picked up automatically via the AWS SDK credential chain)

### Docker image

```
ghcr.io/cowprotocol/memory-monitor:<tag>
```

Tags:
- `main` — latest build from the main branch (used for staging/shadow)
- `latest` — latest release (used for prod)
- `v*` — specific version tags (e.g., `v0.1.0`)
- `sha-*` — specific commit SHA

### Infrastructure integration

The sidecar is wired into Kubernetes deployments via the `createMemoryMonitorSidecar()` factory in the [infrastructure repo](https://github.com/cowprotocol/infrastructure). The weekly release script pins the `latest` tag to a specific version for prod deployments.

## Development

```bash
cargo build                  # Build debug
cargo build --release        # Build release
cargo test --locked          # Run all tests
cargo fmt --check            # Check formatting
cargo clippy -- -D warnings  # Lint (warnings are errors in CI)
```

### Docker

```bash
docker build -t memory-monitor .
```

## Architecture

```
main.rs          — Monitoring loop orchestration
├── config.rs    — Environment variable loading and validation
├── process.rs   — PID lookup and RssAnon reading via /proc
├── history.rs   — Ring buffer with percentile calculation
├── detection.rs — Spike and slow-leak detection with cooldowns
├── heap_dump.rs — jemalloc Unix socket communication
├── s3.rs        — S3 upload with retry
└── slack.rs     — Slack notification with channel routing
```
