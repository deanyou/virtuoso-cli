//! Shared vocabulary for a **hung** SKILL evaluation channel.
//!
//! A hung channel is one where a SKILL evaluation was delivered to Virtuoso
//! and never came back. The evaluation runs inside the SKILL interpreter's
//! IPC callback (`evalstring`), so it cannot be interrupted from the outside:
//! the only thing vcli can do is stop *delivering* new work to that pipe and
//! refuse the requests that follow.
//!
//! Both ends of the wire need the same words for that state, so they live
//! here: the daemon writes them, the client recognises them and passes them
//! through unchanged instead of re-wording them into something vaguer.
//!
//! The wording is deliberately bilingual. The operator who has to act on it
//! is looking at a dead Virtuoso session and needs to know, without reading
//! English, that the channel is gone and the session must be restarted.

/// Machine-readable marker that identifies a hung-channel report.
///
/// The client keys off this marker to tell a hung channel apart from an
/// ordinary SKILL failure.
pub const HUNG_MARKER: &str = "skill_channel_hung";

/// Operator-facing text (Chinese): the channel is dead, restart the session.
pub const HUNG_TEXT_ZH: &str = "通道已挂死，需重启会话";

/// Operator-facing text (English): the same statement, for logs and for
/// operators who do not read Chinese.
pub const HUNG_TEXT_EN: &str = "channel hung, restart session required";

/// True when `message` is (or contains) a hung-channel report.
///
/// Used by the client to recognise the daemon's verdict and by the screenshot
/// command to decide that the SKILL route is beyond recovery.
pub fn is_hung_message(message: &str) -> bool {
    message.contains(HUNG_MARKER)
        || message.contains(HUNG_TEXT_ZH)
        || message.contains(HUNG_TEXT_EN)
}

/// The evaluation that was delivered to Virtuoso and never returned.
///
/// This is the in-flight record the daemon keeps after a timeout: which
/// expression was stuck, when it was delivered, and how long it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StuckCall {
    /// One-line summary of the SKILL text that was delivered.
    pub skill_summary: String,
    /// When the evaluation was delivered (UTC, RFC 3339).
    pub started_at: String,
    /// The same instant as a Unix timestamp.
    pub started_unix: u64,
    /// The deadline that expired.
    pub timeout_secs: u64,
}

impl StuckCall {
    /// Build the record for `skill`, delivered at `started_at`.
    pub fn new(
        skill: &str,
        started_at: impl Into<String>,
        started_unix: u64,
        timeout_secs: u64,
    ) -> Self {
        Self {
            skill_summary: summarize_skill(skill),
            started_at: started_at.into(),
            started_unix,
            timeout_secs,
        }
    }
}

/// One-line summary of a SKILL expression.
///
/// Whitespace is collapsed (the expression may span lines) and the result is
/// truncated, because this text ends up in an error message that a human (or
/// an agent) has to read.
pub fn summarize_skill(skill: &str) -> String {
    const MAX_CHARS: usize = 160;
    let flat = skill.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        flat
    } else {
        let head: String = flat.chars().take(MAX_CHARS).collect();
        format!("{head}...")
    }
}

/// The message the daemon sends when the channel is dead.
///
/// It names the stuck call, states that the evaluation cannot be cancelled
/// from outside the SKILL interpreter, and says what to do about it. The
/// client hands this to the operator unchanged.
pub fn hung_message(stuck: &StuckCall) -> String {
    format!(
        "{marker}: {zh} ({en}).\n\
         Stuck call: skill={skill}\n\
         Stuck call: started_at={started_at} (unix {started_unix}), timeout_secs={timeout}\n\
         The previous SKILL evaluation never returned. Virtuoso may still be running it, and \
         it cannot be cancelled from outside the SKILL interpreter.\n\
         No further SKILL request is written to this channel. Restart the Virtuoso session \
         to recover (RBStop/RBStart in the CIW — but a hung evaluation blocks the CIW too, \
         so restarting Virtuoso itself may be the only way back).",
        marker = HUNG_MARKER,
        zh = HUNG_TEXT_ZH,
        en = HUNG_TEXT_EN,
        skill = stuck.skill_summary,
        started_at = stuck.started_at,
        started_unix = stuck.started_unix,
        timeout = stuck.timeout_secs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stuck() -> StuckCall {
        StuckCall::new(
            "maeOpenSetup(\"LIB\" \"CELL\" \"maestro\")",
            "2026-11-10T22:29:33Z",
            1_762_792_173,
            30,
        )
    }

    #[test]
    fn hung_message_contains_both_languages_and_the_stuck_call() {
        let msg = hung_message(&stuck());
        assert!(
            msg.contains(HUNG_TEXT_ZH),
            "missing the Chinese text: {msg}"
        );
        assert!(
            msg.contains(HUNG_TEXT_EN),
            "missing the English text: {msg}"
        );
        assert!(msg.contains(HUNG_MARKER), "missing the marker: {msg}");
        assert!(msg.contains("maeOpenSetup"), "missing the skill: {msg}");
        assert!(
            msg.contains("2026-11-10T22:29:33Z"),
            "missing the start time: {msg}"
        );
        assert!(msg.contains("30"), "missing the timeout: {msg}");
    }

    #[test]
    fn hung_message_is_recognised_by_the_client_side_check() {
        assert!(is_hung_message(&hung_message(&stuck())));
        assert!(!is_hung_message("TimeoutError"));
        assert!(!is_hung_message("execution failed: nil"));
    }

    #[test]
    fn summarize_skill_collapses_whitespace_and_truncates() {
        assert_eq!(summarize_skill("1 +\n  1"), "1 + 1");
        let long = "x".repeat(400);
        let summary = summarize_skill(&long);
        assert!(summary.chars().count() <= 163, "summary too long");
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn stuck_call_keeps_the_skill_summary() {
        let s = StuckCall::new("a  b", "now", 1, 2);
        assert_eq!(s.skill_summary, "a b");
        assert_eq!(s.started_at, "now");
        assert_eq!(s.started_unix, 1);
        assert_eq!(s.timeout_secs, 2);
    }
}
