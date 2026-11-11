//! `virtuoso-daemon` binary entry point.
//!
//! The behaviour lives in [`virtuoso_cli::daemon`] so that the timeout and
//! hung-channel state machine can be unit-tested without a live Virtuoso
//! session.

fn main() {
    virtuoso_cli::daemon::run_cli();
}
