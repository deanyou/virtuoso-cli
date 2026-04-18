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

**Working approach**: Lazy refresh via `kill(pid, 0)`, with log-file supplement for zombies:
```rust
pub fn refresh(&mut self) {
    if self.status == Running {
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !alive {
            self.finish_from_log();
        } else {
            // kill -0 returns 0 for zombie processes too.
            // Supplement: check log even when PID appears alive.
            if spectre_out.exists() && log_content.contains("completes with") {
                self.finish_from_log();
            }
        }
    }
}
```

Called on `job-status` and `job-list` queries — status updates lazily when checked.

### Critical Caveat: Zombie Processes (discovered 2026-04-18)

`kill(pid, 0)` returns **0 (alive)** for zombie processes — child processes that have
exited but whose parent never called `wait()`. Because `runner.rs` stores only the
PID (not the `Child` handle), it cannot call `child.wait()`, so every spectre job
becomes a zombie after completion.

**Symptom**: `vcli optim run` returns `"failed": N, "completed": 0` even though
`spectre.out` contains `"completes with 0 errors"`.

**Fix applied** (`src/spectre/jobs.rs:refresh()`, 2026-04-18): When `kill -0` says
alive for local jobs, also check whether `spectre.out` contains `"completes with"`.
If yes, call `finish_from_log()` regardless of PID status.

**Root fix** (not yet applied): `runner.rs` should save the `Child` handle (not just
PID) and spawn a thread to call `child.wait()` before exiting, or use `SIGCHLD` handling.

## When to Use
- Any CLI tool that needs fire-and-forget process management
- When the monitoring process is short-lived but the child is long-running
- Alternative: use PID files + cron, or a proper daemon process
