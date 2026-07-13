use crate::client::bridge::VirtuosoClient;
use crate::error::Result;

/// Virtuoso IC version family — determines Maestro SKILL API signatures.
///
/// API 实测结果（2026-04-20, IC25.1 ISR4）：
/// - `maeGetSetup` 仍然返回 list `("setupName")`，`car()` 有效
/// - `maeSetAnalysis` 仍然使用 positional `(setupName type)` 签名
/// - `maeGetEnabledAnalysis` 仍然使用 positional `(setupName)` 签名
///
/// 目前 IC23/IC25 Maestro API 签名完全一致。
/// 版本检测留作基础设施，等未来真正出现不兼容时再启用分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtuosoVersion {
    /// IC23.1 / IC25.1 ISR4 — positional API（当前实测兼容）
    IC23,
    /// 未来的 IC25+ 变体 — 如果 Maestro API 签名真的发生变化
    IC25,
    /// 版本无法确定
    Unknown,
}

impl VirtuosoVersion {
    /// Returns true if this version uses the IC25+ Maestro API signatures.
    /// 当前 IC25.1 ISR4 实测与 IC23 签名一致，返回 false。
    /// 只有真正检测到 API 变化时才返回 true。
    pub fn is_ic25(&self) -> bool {
        // 当前 IC25.1 ISR4 签名与 IC23 一致，不做分支。
        // 如果未来版本出现真正的不兼容，修改这里的判断逻辑。
        false
    }
}

/// Parse the major IC version from a version string like "IC23.1-64b.500" or "IC25.1 ISR1".
pub fn parse_ic_version(version_str: &str) -> VirtuosoVersion {
    // Look for "IC" followed by digits — IC618 = 618, IC23 = 23, IC25 = 25
    let lower = version_str.to_lowercase();
    if let Some(pos) = lower.find("ic") {
        let digits: String = lower[pos + 2..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(major) = digits.parse::<u32>() {
            if major >= 25 {
                return VirtuosoVersion::IC25;
            }
            if major >= 23 {
                return VirtuosoVersion::IC23;
            }
            // IC6.1.x etc. — treat as IC23 (pre-25 API)
            return VirtuosoVersion::IC23;
        }
    }
    VirtuosoVersion::Unknown
}

/// Detect Virtuoso version by querying the daemon.
/// Tries getVersion(t) first (IC23/IC25), falls back to getVersionString() (if available).
pub fn detect_version(client: &VirtuosoClient) -> Result<VirtuosoVersion> {
    // getVersion(t) returns e.g. "sub-version  IC25.1-64b.ISR4.49 "
    // Use execute_skill_unchecked to avoid auth check (internal operation)
    let result = client.execute_skill_unchecked("getVersion(t)", None)?;
    if result.ok() {
        let version_str = result.output.trim().trim_matches('"');
        if !version_str.is_empty() && version_str != "nil" {
            return Ok(parse_ic_version(version_str));
        }
    }
    // Fallback: some builds may have getVersionString()
    let result2 = client.execute_skill_unchecked("getVersionString()", None)?;
    if result2.ok() {
        let version_str = result2.output.trim().trim_matches('"');
        if !version_str.is_empty() && version_str != "nil" {
            return Ok(parse_ic_version(version_str));
        }
    }
    Ok(VirtuosoVersion::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ic23_1_parses_as_ic23() {
        assert_eq!(parse_ic_version("IC23.1-64b.500"), VirtuosoVersion::IC23);
    }

    #[test]
    fn ic25_1_parses_as_ic25() {
        assert_eq!(parse_ic_version("IC25.1 ISR1"), VirtuosoVersion::IC25);
    }

    #[test]
    fn ic618_parses_as_ic23() {
        assert_eq!(parse_ic_version("IC6.1.8-64b.500"), VirtuosoVersion::IC23);
    }

    #[test]
    fn ic24_parses_as_ic23() {
        assert_eq!(parse_ic_version("IC24.1"), VirtuosoVersion::IC23);
    }

    #[test]
    fn empty_string_is_unknown() {
        assert_eq!(parse_ic_version(""), VirtuosoVersion::Unknown);
    }

    #[test]
    fn garbage_is_unknown() {
        assert_eq!(parse_ic_version("foo bar"), VirtuosoVersion::Unknown);
    }
}
