use std::path::Path;
use tracing::warn;

/// Find the PID of a process by its binary name, reading `/proc/[0-9]*/comm`.
fn find_pid(binary_name: &str) -> Option<u32> {
    let proc = Path::new("/proc");
    let entries = std::fs::read_dir(proc).ok()?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Only look at numeric directories
        if !name_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let comm_path = entry.path().join("comm");
        if let Ok(comm) = std::fs::read_to_string(&comm_path) {
            if comm.trim() == binary_name {
                if let Ok(pid) = name_str.parse::<u32>() {
                    return Some(pid);
                }
            }
        }
    }

    None
}

/// Read the RssAnon value from `/proc/<pid>/status` and return it in bytes.
fn read_rss_anon(pid: u32) -> Option<u64> {
    let status_path = format!("/proc/{}/status", pid);
    let contents = std::fs::read_to_string(&status_path).ok()?;

    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("RssAnon:") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] != "kB" {
                warn!(
                    unit = parts[1],
                    "Unexpected unit for RssAnon, expected 'kB'"
                );
            }
            if let Some(kb_str) = parts.first() {
                if let Ok(kb) = kb_str.parse::<u64>() {
                    return Some(kb * 1024);
                }
            }
            return None;
        }
    }

    None
}

/// Get the anonymous RSS memory usage (in bytes) for the process matching `binary_name`.
/// Returns `None` if the process is not found or memory cannot be read.
pub fn get_process_memory(binary_name: &str) -> Option<u64> {
    let pid = find_pid(binary_name)?;
    read_rss_anon(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_pid_nonexistent() {
        // Should return None for a process that doesn't exist
        assert!(find_pid("nonexistent_binary_xyz_12345").is_none());
    }

    #[test]
    fn test_read_rss_anon_invalid_pid() {
        // PID 0 should not have a readable status
        assert!(read_rss_anon(0).is_none());
    }
}
