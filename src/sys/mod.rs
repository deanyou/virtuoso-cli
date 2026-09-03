//! Platform-specific compatibility shims.
//!
//! Targets syscalls or libc symbols that are absent on older distributions
//! (e.g. glibc 2.17 on CentOS 7) but whose semantics can be approximated
//! via a fallback path.

#[cfg(target_os = "linux")]
pub mod linux_rename;
