//! `renameat2(..., RENAME_NOREPLACE)` compatibility for pre-2.28 glibc.
//!
//! The `renameat2` wrapper symbol was added to glibc 2.28. CentOS 7 ships
//! glibc 2.17, so binaries that directly reference the symbol fail to *link*
//! even though the kernel itself (3.10+) supports the `renameat2` syscall.
//! This module checks the loaded libc at runtime and falls back to a
//! non-atomic `stat + renameat` path that preserves the NOREPLACE semantics
//! (destination must not already exist).

use std::ffi::{c_char, c_int, c_uint};
use std::mem;

/// Function-pointer signature matching glibc's renameat2.
#[cfg(target_os = "linux")]
type Renameat2Fn = unsafe extern "C" fn(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
    flags: c_uint,
) -> c_int;

/// Flag value — defined independently of libc to avoid version-gated imports.
#[cfg(target_os = "linux")]
const RENAME_NOREPLACE: c_uint = 1;

/// Drop-in replacement for `renameat2(AT_FDCWD, src, AT_FDCWD, dst, RENAME_NOREPLACE)`
/// that runs on glibc 2.17+.
///
/// Resolution order (paths 2 and 3 exist for platforms where the symbol is
/// missing from libc, e.g. CentOS 7's glibc 2.17):
///   1. `dlsym(RTLD_DEFAULT, "renameat2")` — linked-in and exported, use directly.
///   2. `syscall(SYS_renameat2, ...)` — glibc lacks the wrapper but kernel
///      supports the syscall (production kernels we target ≥ 3.15; many
///      CentOS 7 kernels also satisfy this via backport).
///   3. `lstat64` existence check + `renameat` — last-resort fallback that
///      preserves the "must not overwrite an existing destination" contract
///      at the cost of a small non-atomic window.
#[cfg(target_os = "linux")]
pub unsafe fn rename_noreplace(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
) -> c_int {
    // ---- Fast path: libc exports renameat2 ---
    let sym = unsafe { libc::dlsym(libc::RTLD_DEFAULT, b"renameat2\0".as_ptr() as *const c_char) };
    if !sym.is_null() {
        let f: Renameat2Fn = unsafe { mem::transmute(sym) };
        let res = unsafe { f(olddirfd, oldpath, newdirfd, newpath, RENAME_NOREPLACE) };
        if res == 0 {
            return 0;
        }
        return res;
    }

    // ---- Medium path: raw syscall (glibc < 2.28, kernel >= 3.15) ----
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    let sys_num: libc::c_long = libc::SYS_renameat2 as libc::c_long;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let sys_num: libc::c_long = -1; // sentinel: unknown arch — skip to fallback

    if sys_num > 0 {
        let pad: libc::c_long = 0; // renameat2 really takes 5 args; pad to 6
        let res = unsafe {
            libc::syscall(
                sys_num,
                olddirfd,
                oldpath,
                newdirfd,
                newpath,
                RENAME_NOREPLACE,
                pad,
            )
        };
        if res == 0 {
            return 0;
        }
        let en = negate_err(res);
        if libc::ENOSYS != en {
            // A real failure (EEXIST, ENOTEMPTY, EACCES…). Return and let the
            // caller read errno.
            return -1;
        }
        // ENOSYS means the syscall instruction is unsupported; the path below
        // handles it. There is no arch with syscalls but no errno write, so
        // the kernel has already set errno for us above via the libc crate's
        // internal mechanism.
    }

    // ---- Slow path: lstat + renameat (present since glibc 2.0) ----
    let mut st: libc::stat64 = unsafe { mem::zeroed() };
    let exists = unsafe { libc::lstat64(newpath, &mut st) } == 0;
    if exists {
        unsafe { *errno_location() = libc::EEXIST };
        return -1;
    }

    unsafe { libc::renameat(olddirfd, oldpath, newdirfd, newpath) }
}

/// Negate a raw syscall result to get the positive errno.
#[cfg(target_os = "linux")]
fn negate_err(code: libc::c_long) -> c_int {
    if code < 0 && code > -4096 {
        (-code) as c_int
    } else {
        0
    }
}

#[cfg(target_os = "linux")]
unsafe fn errno_location() -> *mut c_int {
    libc::__errno_location()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs;

    fn make_pair() -> (CString, CString) {
        let pid = std::process::id();
        let src = CString::new(format!("/tmp/_virtuoso_test_src_{}", pid)).unwrap();
        let dst = CString::new(format!("/tmp/_virtuoso_test_dst_{}", pid)).unwrap();
        let _ = fs::remove_file(src.to_str().unwrap());
        let _ = fs::remove_file(dst.to_str().unwrap());
        fs::write(src.to_str().unwrap(), b"hello").unwrap();
        (src, dst)
    }

    #[test]
    fn rename_noreplace_basic() {
        let (src, dst) = make_pair();
        let (sp, dp) = (src.as_ptr(), dst.as_ptr());

        let rc = unsafe { rename_noreplace(libc::AT_FDCWD, sp, libc::AT_FDCWD, dp) };
        assert_eq!(rc, 0, "first rename should succeed, errno={}", unsafe {
            *errno_location()
        });

        assert_eq!(fs::read_to_string(dst.to_str().unwrap()).unwrap(), "hello");
        assert!(!std::path::Path::new(src.to_str().unwrap()).exists());

        let rc = unsafe { rename_noreplace(libc::AT_FDCWD, dp, libc::AT_FDCWD, dp) };
        assert_eq!(rc, -1);
        assert_eq!(unsafe { *errno_location() }, libc::EEXIST);

        let _ = fs::remove_file(dst.to_str().unwrap());
    }

    #[test]
    fn rename_noreplace_missing_src() {
        let pid = std::process::id();
        let missing = CString::new(format!("/tmp/_virtuoso_missing_{}", pid)).unwrap();
        let dst = CString::new(format!("/tmp/_virtuoso_dst_missing_{}", pid)).unwrap();
        let _ = fs::remove_file(missing.to_str().unwrap());
        let _ = fs::remove_file(dst.to_str().unwrap());

        let rc = unsafe {
            rename_noreplace(
                libc::AT_FDCWD,
                missing.as_ptr(),
                libc::AT_FDCWD,
                dst.as_ptr(),
            )
        };
        assert_eq!(rc, -1);
        assert_eq!(unsafe { *errno_location() }, libc::ENOENT);

        let _ = fs::remove_file(dst.to_str().unwrap());
    }

    #[test]
    fn negate_err_sanity() {
        assert_eq!(negate_err(-42), 42);
        assert_eq!(negate_err(0), 0);
        assert_eq!(negate_err(100), 0);
        // Out-of-range negative (not an errno) → 0
        assert_eq!(negate_err(-5000), 0);
    }
}
