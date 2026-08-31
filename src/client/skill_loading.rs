//! Staging for SKILL files uploaded through an SSH-backed bridge.
use crate::error::{Result, VirtuosoError};
use crate::transport::contract::{CommandRequest, RemoteTransport, UploadFileRequest};
use crate::transport::ssh::shell_quote;
use std::path::Path;

pub(super) fn stage_remote_file(transport: &dyn RemoteTransport, local: &Path) -> Result<String> {
    let filename = local
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| VirtuosoError::Config("SKILL filename must be UTF-8".into()))?;

    // The tar-based transport archives symlinks rather than following them.
    // Materialize their bytes under the original name so .ils still selects
    // SKILL++, and never resolve a local link against the remote filesystem.
    let staging = if local.is_symlink() {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join(filename);
        std::fs::copy(local, &source)?;
        Some((directory, source))
    } else {
        None
    };
    let source = staging.as_ref().map_or(local, |(_, path)| path.as_path());

    // mktemp creates a private, unique directory owned by the SSH user. Do not
    // depend on /tmp/virtuoso_bridge, which may belong to another Unix user.
    // Run through sh explicitly because the SSH login shell may be csh.
    let command = format!(
        "sh -c {}",
        shell_quote("umask 077; mktemp -d \"${TMPDIR:-/tmp}/vcli-skill-XXXXXXXXXX\"")
    );
    let result = transport.run_command(&CommandRequest::untimed(command))?;
    if !result.success || result.exit_status != 0 {
        return Err(VirtuosoError::Ssh(format!(
            "create remote SKILL directory failed (exit {}): {}",
            result.exit_status,
            result.stderr.trim()
        )));
    }
    let directory = result.stdout.trim_end_matches(['\r', '\n']);
    if !directory.starts_with('/') || directory.contains(['\r', '\n', '\0']) {
        return Err(VirtuosoError::Ssh(
            "mktemp did not return a single absolute remote SKILL directory".into(),
        ));
    }
    // Keep the original suffix: .ils selects SKILL++ rather than SKILL.
    let remote = format!("{directory}/{filename}");
    transport.upload_file(&UploadFileRequest::untimed(source, &remote))?;
    // Retain the source for debugging and later callbacks, as load_il did before.
    Ok(remote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::test_support::FakeTransport;

    #[cfg(unix)]
    #[test]
    fn remote_symlink_uploads_source_bytes_with_original_language_suffix() {
        use crate::transport::contract::{
            CommandResult, Deadline, DownloadDirRequest, DownloadFileRequest, TransportError,
            UploadTextRequest,
        };
        struct InspectUpload(FakeTransport);
        impl RemoteTransport for InspectUpload {
            fn test_connection(&self, _: Deadline) -> std::result::Result<bool, TransportError> {
                unreachable!()
            }
            fn run_command(
                &self,
                req: &CommandRequest,
            ) -> std::result::Result<CommandResult, TransportError> {
                self.0.run_command(req)
            }
            fn upload_file(
                &self,
                req: &UploadFileRequest,
            ) -> std::result::Result<(), TransportError> {
                if std::fs::symlink_metadata(&req.local)
                    .unwrap()
                    .file_type()
                    .is_symlink()
                {
                    return Err(TransportError::LocalIo(
                        "upload would archive a symlink".into(),
                    ));
                }
                assert_eq!(
                    std::fs::read(&req.local).unwrap(),
                    b"procedure(probe() 42)\nt\n"
                );
                assert_eq!(req.local.file_name().unwrap(), "probe.ils");
                assert!(req.remote.ends_with("/probe.ils"));
                Ok(())
            }
            fn upload_text(
                &self,
                _: &UploadTextRequest,
            ) -> std::result::Result<(), TransportError> {
                unreachable!()
            }
            fn download_file(
                &self,
                _: &DownloadFileRequest,
            ) -> std::result::Result<(), TransportError> {
                unreachable!()
            }
            fn download_dir(
                &self,
                _: &DownloadDirRequest,
            ) -> std::result::Result<(), TransportError> {
                unreachable!()
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.il");
        std::fs::write(&target, b"procedure(probe() 42)\nt\n").unwrap();
        let link = directory.path().join("probe.ils");
        std::os::unix::fs::symlink("target.il", &link).unwrap();
        let mut fake = FakeTransport::ok();
        fake.command_result.stdout = "/tmp/vcli-skill-unique\n".into();
        stage_remote_file(&InspectUpload(fake), &link).unwrap();
    }

    #[test]
    fn remote_staging_preserves_filename_and_ils_suffix() {
        let mut transport = FakeTransport::ok();
        transport.command_result.stdout = "/tmp/vcli-skill-unique\n".into();
        assert_eq!(
            stage_remote_file(&transport, Path::new("/local/probe with spaces.ils")).unwrap(),
            "/tmp/vcli-skill-unique/probe with spaces.ils"
        );
    }

    #[test]
    fn remote_directory_failure_preserves_stderr() {
        let mut transport = FakeTransport::ok();
        transport.command_result.success = false;
        transport.command_result.exit_status = 1;
        transport.command_result.stderr = "Permission denied".into();
        let error = stage_remote_file(&transport, Path::new("/local/probe.il")).unwrap_err();
        assert!(matches!(error, VirtuosoError::Ssh(_)));
        assert!(error.to_string().contains("Permission denied"));
    }

    #[test]
    fn remote_directory_must_be_one_absolute_path() {
        for output in ["", "relative", "banner\n/tmp/vcli-skill-test\n"] {
            let mut transport = FakeTransport::ok();
            transport.command_result.stdout = output.into();
            assert!(stage_remote_file(&transport, Path::new("/local/probe.il")).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn remote_mktemp_command_creates_private_unique_directories() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("remote tmp with ' quotes");
        std::fs::create_dir(&root).unwrap();
        let mut transport = FakeTransport::ok();
        transport.command_result.stdout = "/tmp/vcli-skill-test\n".into();
        stage_remote_file(&transport, Path::new("/local/probe.il")).unwrap();
        let command = &transport.commands.lock().unwrap()[0];
        let mut directories = Vec::new();
        for _ in 0..2 {
            let output = std::process::Command::new("sh")
                .args(["-c", command])
                .env("TMPDIR", &root)
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output);
            let path = std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
            assert!(path.starts_with(&root));
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            directories.push(path);
        }
        assert_ne!(directories[0], directories[1]);
    }
}
