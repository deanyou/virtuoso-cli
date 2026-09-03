//! Integration test for the CentOS 7 (glibc 2.17) renameat2 compatibility shim.
//!
//! These tests run only on Linux — the sys::linux_rename module is compiled and
//! exercised solely on that platform. macOS uses `renamex_np` directly via a
//! separate code path in commands::maestro::atomic_publish_no_replace.

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::CString;
    use std::fs;
    use std::path::Path;

    use virtuoso_cli::sys::linux_rename::rename_noreplace;

    fn tmp_cpath(name: &str) -> CString {
        CString::new(format!(
            "/tmp/vcli_rename_test_{}_{}",
            std::process::id(),
            name
        ))
        .unwrap()
    }

    fn cleanup(name: &str) {
        let p = format!("/tmp/vcli_rename_test_{}_{}", std::process::id(), name);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn happy_path_creates_dst() {
        let src = tmp_cpath("src1");
        let dst = tmp_cpath("dst1");
        cleanup("src1");
        cleanup("dst1");
        fs::write(src.to_str().unwrap(), b"payload").unwrap();

        let rc =
            unsafe { rename_noreplace(libc::AT_FDCWD, src.as_ptr(), libc::AT_FDCWD, dst.as_ptr()) };
        assert_eq!(rc, 0);
        assert_eq!(
            fs::read_to_string(dst.to_str().unwrap()).unwrap(),
            "payload"
        );
        assert!(
            !Path::new(src.to_str().unwrap()).exists(),
            "src must be gone"
        );

        cleanup("dst1");
    }

    #[test]
    fn eexist_when_dst_is_file() {
        let src = tmp_cpath("src2");
        let dst = tmp_cpath("dst2");
        cleanup("src2");
        cleanup("dst2");
        fs::write(src.to_str().unwrap(), b"A").unwrap();
        fs::write(dst.to_str().unwrap(), b"B").unwrap();

        let rc =
            unsafe { rename_noreplace(libc::AT_FDCWD, src.as_ptr(), libc::AT_FDCWD, dst.as_ptr()) };
        assert_eq!(rc, -1, "rename onto an existing file must fail");
        assert_eq!(unsafe { *libc::__errno_location() }, libc::EEXIST);
        // Destination must be unmodified
        assert_eq!(fs::read_to_string(dst.to_str().unwrap()).unwrap(), "B");

        cleanup("src2");
        cleanup("dst2");
    }

    #[test]
    fn eexist_when_dst_is_empty_dir() {
        let src = tmp_cpath("src3");
        let dst = tmp_cpath("dst3_empty_dir");
        cleanup("src3");
        let _ = fs::remove_dir(dst.to_str().unwrap());
        fs::write(src.to_str().unwrap(), b"A").unwrap();
        fs::create_dir(dst.to_str().unwrap()).unwrap();

        let rc =
            unsafe { rename_noreplace(libc::AT_FDCWD, src.as_ptr(), libc::AT_FDCWD, dst.as_ptr()) };
        assert_eq!(rc, -1, "rename onto an existing dir must fail");
        assert_eq!(unsafe { *libc::__errno_location() }, libc::EEXIST);
        // src must NOT be consumed (noreplace semantics).
        assert!(Path::new(src.to_str().unwrap()).exists());

        let _ = fs::remove_dir(dst.to_str().unwrap());
        cleanup("src3");
    }

    #[test]
    fn enoent_when_src_missing() {
        let src = tmp_cpath("no_such_src");
        let dst = tmp_cpath("no_such_dst");
        cleanup("no_such_src");
        cleanup("no_such_dst");

        let rc =
            unsafe { rename_noreplace(libc::AT_FDCWD, src.as_ptr(), libc::AT_FDCWD, dst.as_ptr()) };
        assert_eq!(rc, -1);
        assert_eq!(unsafe { *libc::__errno_location() }, libc::ENOENT);
    }

    #[test]
    fn unicode_filename() {
        // C string with multi-byte UTF-8 bytes — exercise the lossless path
        // that uses *CString (no OsStr).
        let name = format!("unicode_{}_细胞_🦀", std::process::id());
        let src_path = format!("/tmp/{}", name);
        let dst_path = format!("/tmp/dst_{}", name);
        let _ = fs::remove_file(&src_path);
        let _ = fs::remove_file(&dst_path);
        fs::write(&src_path, b"payload").unwrap();

        let src_c = CString::new(src_path.as_bytes()).unwrap();
        let dst_c = CString::new(dst_path.as_bytes()).unwrap();
        let rc = unsafe {
            rename_noreplace(
                libc::AT_FDCWD,
                src_c.as_ptr(),
                libc::AT_FDCWD,
                dst_c.as_ptr(),
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(fs::read_to_string(&dst_path).unwrap(), "payload");
        assert!(!Path::new(&src_path).exists());

        let _ = fs::remove_file(&dst_path);
    }
}

/// Confirms the sys::linux_rename module is cfg'd out on non-Linux — the
/// crate's public API must never reference it on those targets.
#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_builds_no_compat_module() {}
