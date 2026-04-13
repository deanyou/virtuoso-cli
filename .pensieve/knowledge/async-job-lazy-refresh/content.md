# Async job status: lazy refresh, not monitor threads

## Source
Implementing `vcli sim run-async` — monitor thread approach failed because CLI process exits immediately.

## Summary
Short-lived CLI processes cannot use background threads to monitor child processes. Use lazy status checking instead.

## Content
When a CLI tool (like vcli) spawns a background process and exits immediately, any
monitor thread spawned to `child.wait()` dies with the parent process. The child
process (spectre) runs fine, but the job status file never gets updated.

**Failed approach**: `std::thread::spawn(move || { child.wait(); update_job_status(); })`
— thread dies when vcli exits.

**Working approach**: Lazy refresh via `kill(pid, 0)`:
```rust
pub fn refresh(&mut self) {
    if self.status == Running {
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !alive {
            // Parse spectre.out log to determine success/failure
            self.status = if log.contains("0 errors") { Completed } else { Failed };
            self.save();
        }
    }
}
```

Called on `job-status` and `job-list` queries — status updates lazily when checked.

## When to Use
- Any CLI tool that needs fire-and-forget process management
- When the monitoring process is short-lived but the child is long-running
- Alternative: use PID files + cron, or a proper daemon process
