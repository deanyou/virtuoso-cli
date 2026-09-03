"""vgui_runner.trace - Append-only JSONL event logging with immediate flush."""

import json
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional


@dataclass
class Event:
    seq: int
    ts: str
    state: str
    step_id: Optional[str]
    attempt: int
    outcome: Optional[str]
    duration_ms: Optional[int]
    details: Optional[dict]

    def to_json(self) -> str:
        return json.dumps(asdict(self), separators=(",", ":"))


class Trace:
    """Append-only JSONL event log with immediate flush."""

    def __init__(self, path: Path):
        self._path = path
        self._seq = 0
        self._file = None

    def _open(self) -> None:
        if self._file is None:
            self._file = open(self._path, "a", encoding="utf-8")

    def emit(
        self,
        state: str,
        step_id: Optional[str] = None,
        attempt: int = 0,
        outcome: Optional[str] = None,
        duration_ms: Optional[int] = None,
        details: Optional[dict] = None,
    ) -> Event:
        self._open()
        event = Event(
            seq=self._seq,
            ts=datetime.now(timezone.utc).isoformat(),
            state=state,
            step_id=step_id,
            attempt=attempt,
            outcome=outcome,
            duration_ms=duration_ms,
            details=details,
        )
        line = event.to_json() + "\n"
        self._file.write(line)
        self._file.flush()
        self._seq += 1
        return event

    def close(self) -> None:
        if self._file is not None:
            self._file.close()
            self._file = None
