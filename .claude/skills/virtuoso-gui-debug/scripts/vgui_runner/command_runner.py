"""vgui_runner.command_runner — fixed-argv subprocess transport (Task 3).

Security contract (2026-09-01 live-executor design):
- every command runs as ``subprocess.run(list(argv), shell=False, ...)``;
- the child environment is cleaned to PATH/LANG/LC_ALL plus an explicit
  ``VCLI_CAPABILITY=admin``;
- the SSH adapter only executes a *fixed* vcli argv: argv[0] must be the
  configured vcli path, and every element is safely shell-quoted for the
  remote side — no arbitrary remote commands are accepted;
- argv elements containing NUL or newline are rejected before execution;
- timeouts produce a structured ``CommandResult(timed_out=True)``.
"""

import re
import shlex
import subprocess
import time
from dataclasses import dataclass
from typing import List, Optional, Sequence

__all__ = [
    "CommandError",
    "CommandResult",
    "CommandRunner",
    "LocalRunner",
    "SshRunner",
]

# Keys kept from the parent environment. Everything else (credentials,
# license paths, DISPLAY, VB_*) is dropped before spawning a child.
_ENV_ALLOWLIST = ("PATH", "LANG", "LC_ALL")
_EXTRA_ENV = {"VCLI_CAPABILITY": "admin"}

# A syntactically safe SSH hostname: no whitespace, shell metacharacters,
# NUL, or newlines.
_HOST_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")


class CommandError(Exception):
    """Structured rejection of an unsafe invocation.

    ``message`` is safe to log: it never embeds raw argv values that may
    carry typed input text.
    """

    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason

    def to_dict(self) -> dict:
        return {"error": self.reason}


@dataclass(frozen=True)
class CommandResult:
    """Outcome of one fixed-argv command."""

    exit_code: Optional[int]
    stdout: str
    stderr: str
    duration_ms: int
    timed_out: bool


def _clean_env() -> dict:
    env = {}
    for k in _ENV_ALLOWLIST:
        os_value = _getenv(k)
        if os_value is not None:
            env[k] = os_value
    env.update(_EXTRA_ENV)
    return env


def _getenv(key: str) -> Optional[str]:
    import os

    return os.environ.get(key)


def _validate_argv(argv: Sequence[str]) -> List[str]:
    if isinstance(argv, (str, bytes)):
        raise CommandError("argv must be a sequence of strings, not a shell string")
    items = list(argv)
    if not items:
        raise CommandError("argv must not be empty")
    for item in items:
        if not isinstance(item, str):
            raise CommandError("argv elements must be strings")
        if "\x00" in item:
            raise CommandError("argv element contains NUL")
        if "\n" in item or "\r" in item:
            raise CommandError("argv element contains a newline")
    return items


class CommandRunner:
    """Base class: runs a fixed argv with a timeout and cleaned env."""

    def __init__(self, vcli_path: str):
        self.vcli_path = vcli_path

    def run(self, argv: Sequence[str], timeout_seconds: int) -> CommandResult:
        items = _validate_argv(argv)
        return self._run_validated(items, timeout_seconds)

    def _run_validated(self, argv: List[str], timeout_seconds: int) -> CommandResult:
        raise NotImplementedError


class LocalRunner(CommandRunner):
    """Runs the fixed argv locally with shell=False and a cleaned env."""

    def _run_validated(self, argv: List[str], timeout_seconds: int) -> CommandResult:
        start = time.monotonic()
        try:
            proc = subprocess.run(
                argv,
                shell=False,
                timeout=timeout_seconds,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                universal_newlines=True,
                env=_clean_env(),
            )
            duration_ms = int((time.monotonic() - start) * 1000)
            return CommandResult(
                exit_code=proc.returncode,
                stdout=proc.stdout,
                stderr=proc.stderr,
                duration_ms=duration_ms,
                timed_out=False,
            )
        except subprocess.TimeoutExpired:
            duration_ms = int((time.monotonic() - start) * 1000)
            return CommandResult(
                exit_code=None,
                stdout="",
                stderr=f"command timed out after {timeout_seconds}s",
                duration_ms=duration_ms,
                timed_out=True,
            )


class SshRunner(CommandRunner):
    """Runs a *fixed vcli argv* on a remote host over SSH.

    Only the configured vcli path may be argv[0]; every element is safely
    shell-quoted into a single remote command. This adapter never accepts
    arbitrary remote commands.
    """

    def __init__(self, vcli_path: str, ssh_host: str):
        super().__init__(vcli_path)
        if not isinstance(ssh_host, str) or not ssh_host:
            raise CommandError("ssh_host must be a non-empty string")
        if "\x00" in ssh_host or "\n" in ssh_host or "\r" in ssh_host:
            raise CommandError("ssh_host contains forbidden characters")
        if not _HOST_RE.match(ssh_host):
            raise CommandError(
                "ssh_host must be a plain hostname "
                "(alphanumerics, dots, underscores, hyphens)"
            )
        self.ssh_host = ssh_host

    def ssh_argv(self, vcli_argv: Sequence[str]) -> List[str]:
        """Build the fixed local ssh argv for a vcli command.

        Returns ``["ssh", "--", <host>, <quoted remote command>]`` where the
        remote command is the safely quoted vcli argv.
        """
        items = _validate_argv(vcli_argv)
        if items[0] != self.vcli_path:
            raise CommandError(
                "SSH mode only executes the fixed vcli argv; "
                f"argv[0] must be {self.vcli_path!r}"
            )
        remote = " ".join(shlex.quote(item) for item in items)
        return ["ssh", "--", self.ssh_host, remote]

    def _run_validated(self, argv: List[str], timeout_seconds: int) -> CommandResult:
        full_argv = self.ssh_argv(argv)
        start = time.monotonic()
        try:
            proc = subprocess.run(
                full_argv,
                shell=False,
                timeout=timeout_seconds,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                universal_newlines=True,
                env=_clean_env(),
            )
            duration_ms = int((time.monotonic() - start) * 1000)
            return CommandResult(
                exit_code=proc.returncode,
                stdout=proc.stdout,
                stderr=proc.stderr,
                duration_ms=duration_ms,
                timed_out=False,
            )
        except subprocess.TimeoutExpired:
            duration_ms = int((time.monotonic() - start) * 1000)
            return CommandResult(
                exit_code=None,
                stdout="",
                stderr=f"ssh command timed out after {timeout_seconds}s",
                duration_ms=duration_ms,
                timed_out=True,
            )
