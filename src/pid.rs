use std::fs;
use std::io::{self, ErrorKind};
use std::path::PathBuf;

/// Get the PID file path
pub fn get_pid_file() -> PathBuf {
    // Use /tmp to avoid Docker mount persistence issues.
    // PID files are only meaningful within a single container lifecycle.
    PathBuf::from("/tmp/ccm.pid")
}

/// Write the current process PID to the PID file
pub fn write_pid() -> io::Result<()> {
    let pid_file = get_pid_file();

    // Create parent directory if it doesn't exist
    if let Some(parent) = pid_file.parent() {
        fs::create_dir_all(parent)?;
    }

    let pid = std::process::id();
    fs::write(&pid_file, pid.to_string())?;
    tracing::info!("PID {} written to {:?}", pid, pid_file);
    Ok(())
}

/// Read the PID from the PID file
pub fn read_pid() -> io::Result<u32> {
    let pid_file = get_pid_file();
    let pid_str = fs::read_to_string(&pid_file)?;
    pid_str.trim().parse::<u32>()
        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

/// Remove the PID file
pub fn cleanup_pid() -> io::Result<()> {
    let pid_file = get_pid_file();
    if pid_file.exists() {
        fs::remove_file(&pid_file)?;
        tracing::info!("PID file removed: {:?}", pid_file);
    }
    Ok(())
}

/// Check if a process is running
/// Returns false if PID matches the current process (self-check).
#[cfg(unix)]
pub fn is_process_running(pid: u32) -> bool {
    if pid == std::process::id() {
        return false; // Can't be "already running" if it's us
    }
    if pid == 0 {
        return false; // PID 0 is reserved (system idle), never "running"
    }
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    kill(Pid::from_raw(pid as i32), Signal::SIGCONT).is_ok()
}

#[cfg(windows)]
pub fn is_process_running(pid: u32) -> bool {
    if pid == std::process::id() {
        return false;
    }
    use std::process::Command;

    Command::new("tasklist")
        .args(&["/FI", &format!("PID eq {}", pid)])
        .output()
        .ok()
        .and_then(|output| {
            String::from_utf8(output.stdout)
                .ok()
                .map(|s| s.contains(&pid.to_string()))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_pid_returns_false() {
        let cur = std::process::id();
        assert!(!is_process_running(cur), "current PID should not be considered 'running'");
    }

    #[test]
    fn test_impossible_pid_returns_false() {
        // PID 1 is init/systemd, always running on Linux
        // Use 999_999_999u32 which cannot exist on any real system
        assert!(!is_process_running(999_999_999u32), "999_999_999u32 should not be considered 'running'");
    }
}
