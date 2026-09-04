"""vgui_runner.model - Strict scenario parsing with deeply immutable dataclasses."""

import re
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from types import MappingProxyType
from typing import Any, Dict, List, Optional, Tuple


SUPPORTED_VERSION = "1.0"


class Operation(str, Enum):
    VCLI_LOAD = "VCLI_LOAD"
    VCLI_CALL = "VCLI_CALL"
    WINDOW_WAIT = "WINDOW_WAIT"
    WINDOW_ACTIVATE = "WINDOW_ACTIVATE"
    WINDOW_DISCOVER = "WINDOW_DISCOVER"
    DISMISS_DIALOG = "DISMISS_DIALOG"
    CLOSE = "CLOSE"
    KEY = "KEY"
    TYPE = "TYPE"
    CLICK_REL = "CLICK_REL"
    CLICK_ABS = "CLICK_ABS"
    DOUBLE_CLICK = "DOUBLE_CLICK"
    DRAG_REL = "DRAG_REL"
    SCROLL = "SCROLL"
    MINIMIZE = "MINIMIZE"
    MAXIMIZE = "MAXIMIZE"
    CIW_INPUT = "CIW_INPUT"
    SCREENSHOT = "SCREENSHOT"
    VERIFY = "VERIFY"
    RECOVER = "RECOVER"


ALLOWED_OPERATIONS = {op.value for op in Operation}

# Required and optional argument keys for each operation.
# The verifier and rollback use VERIFY and RECOVER respectively.
# DRAG_REL: model tracks x1,y1,x2,y2 at scenario level. Rust cli accepts
# x,y only — each drag-rel is translated into a mousedown → mousemove(x,y)
# → mouseup sequence at execution time. The REL coordinates are relative to
# the window origin, not screen-wide (that's why CLICK_REL/DRAG_REL naming).
OP_ARG_SCHEMAS = {
    Operation.VCLI_LOAD: ({"command"}, {"skillpp"}),
    Operation.VCLI_CALL: ({"function", "args"}, {"kwargs"}),
    Operation.WINDOW_WAIT: ({"window_title", "state"}, set()),
    Operation.WINDOW_ACTIVATE: ({"window_title"}, set()),
    Operation.WINDOW_DISCOVER: (set(), {"title", "class", "pid"}),
    Operation.DISMISS_DIALOG: (set(), {"window_title", "window_id"}),
    Operation.CLOSE: (set(), {"window_id"}),
    Operation.KEY: ({"keys"}, set()),
    Operation.TYPE: ({"text"}, set()),
    Operation.CLICK_REL: ({"x", "y"}, {"button"}),
    Operation.CLICK_ABS: ({"x", "y"}, {"button"}),
    Operation.DOUBLE_CLICK: ({"x", "y"}, {"button"}),
    Operation.DRAG_REL: ({"x", "y"}, {"button"}),
    Operation.SCROLL: ({"direction"}, {"count", "x", "y"}),
    Operation.MINIMIZE: (set(), set()),
    Operation.MAXIMIZE: (set(), set()),
    Operation.CIW_INPUT: ({"text"}, {"delay_ms", "clear_first"}),
    Operation.SCREENSHOT: (set(), {"path"}),
    Operation.VERIFY: ({"predicate", "expected"}, set()),
    Operation.RECOVER: ({"action", "target"}, set()),
}

# Integer-typed argument keys (strict int, bool rejected, must be positive)
INT_ARG_KEYS = {
    Operation.CLICK_REL: {"x", "y"},
    Operation.CLICK_ABS: {"x", "y"},
    Operation.DOUBLE_CLICK: {"x", "y"},
    Operation.DRAG_REL: {"x", "y"},
    Operation.SCROLL: {"count", "x", "y"},
    Operation.CIW_INPUT: {"delay_ms"},
    Operation.WINDOW_DISCOVER: {"pid"},
}

# Button must be positive 1/2/3 — tracked separately so validation runs
# before execution. Unlike coords, button is Optional (default 1 = left).
BUTTON_ARG_KEYS = {
    Operation.CLICK_REL: {"button"},
    Operation.CLICK_ABS: {"button"},
    Operation.DOUBLE_CLICK: {"button"},
    Operation.DRAG_REL: {"button"},
}

# WINDOW_WAIT state allowlist — only these explicit states are supported.
# Any other value (including typos like "visibile") is rejected at parse time.
WINDOW_WAIT_ALLOWED_STATES = {"visible", "hidden"}

# SCROLL direction allowlist — maps to xdotool mouse buttons 4/5/6/7.
SCROLL_ALLOWED_DIRECTIONS = {"up", "down", "left", "right"}

# Supported verifier predicates. Each maps to a concrete vcli query in the
# LiveExecutor verify phase. Unknown predicates fail closed at parse time.
SUPPORTED_PREDICATES = {
    "window_exists", "state_matches", "title_matches", "geometry_matches",
    "ciw_eval",
}

# String-typed argument keys
STR_ARG_KEYS = {
    Operation.VCLI_LOAD: {"command"},
    Operation.VCLI_CALL: {"function"},
    Operation.WINDOW_WAIT: {"window_title", "state"},
    Operation.WINDOW_ACTIVATE: {"window_title"},
    Operation.WINDOW_DISCOVER: {"title", "class"},
    Operation.DISMISS_DIALOG: {"window_title", "window_id"},
    Operation.CLOSE: {"window_id"},
    Operation.KEY: {"keys"},
    Operation.TYPE: {"text"},
    Operation.CIW_INPUT: {"text"},
    Operation.SCROLL: {"direction"},
    Operation.SCREENSHOT: {"path"},
    Operation.VERIFY: {"predicate"},
    Operation.RECOVER: {"action", "target"},
}

# Argument keys whose value must be a JSON array of JSON values
LIST_ARG_KEYS = {
    Operation.VCLI_CALL: {"args"},
}

# Argument keys whose value must be a JSON object of str->JSON value
DICT_ARG_KEYS = {
    Operation.VCLI_CALL: {"kwargs"},
}

# For VERIFY operation: expected can be any JSON value (str, int, float, bool, list, dict, null)
# For RECOVER operation: action and target are strings.

MIN_TIMEOUT = 1
MAX_TIMEOUT = 300
MIN_RETRIES = 0
MAX_RETRIES = 1


def _is_strict_int(x: Any) -> bool:
    """Return True only if x is an integer but NOT a bool."""
    return isinstance(x, int) and not isinstance(x, bool)


def _is_json_value(x: Any) -> bool:
    """Return True if x is a valid JSON-serializable value (recursively)."""
    if x is None:
        return True
    if isinstance(x, bool):
        return True
    if isinstance(x, int):
        return True
    if isinstance(x, float):
        return True
    if isinstance(x, str):
        return True
    if isinstance(x, list):
        return all(_is_json_value(item) for item in x)
    if isinstance(x, dict):
        return all(isinstance(k, str) and _is_json_value(v) for k, v in x.items())
    return False


def _freeze_json(value: Any) -> Any:
    """Recursively freeze JSON values into immutable structures."""
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, list):
        return tuple(_freeze_json(item) for item in value)
    if isinstance(value, dict):
        return MappingProxyType({k: _freeze_json(v) for k, v in value.items()})
    raise TypeError(f"value is not JSON-serializable: {type(value).__name__}")


class ScenarioValidationError(ValueError):
    """Raised when scenario JSON fails validation."""

    def __init__(self, message: str, path: Optional[str] = None):
        self.path = path or "root"
        full = f"{self.path}: {message}" if path else message
        super().__init__(full)


@dataclass(frozen=True)
class CellView:
    lib: str
    cell: str
    view: str

    def __str__(self) -> str:
        return f"{self.lib}/{self.cell}/{self.view}"


@dataclass(frozen=True)
class Step:
    id: str
    operation: Operation
    arguments: MappingProxyType
    verifier: MappingProxyType
    timeout_seconds: int
    max_retries: int
    rollback: Optional[MappingProxyType]

    @staticmethod
    def _check_arguments(args: Dict[str, Any], operation: Operation, step_id: str) -> None:
        path = f"steps[{step_id}].arguments"
        req, opt = OP_ARG_SCHEMAS.get(operation, (set(), set()))
        allowed = req | opt
        unknown = set(args.keys()) - allowed
        if unknown:
            raise ScenarioValidationError(
                f"unknown argument(s): {', '.join(sorted(unknown))}",
                path,
            )
        missing = req - set(args.keys())
        if missing:
            raise ScenarioValidationError(
                f"missing required argument(s): {', '.join(sorted(missing))}",
                path,
            )

        # Validate integer types (strict int, bool rejected). x/y coords may
        # be zero; only the button arg is further constrained to 1..3 below.
        for key in INT_ARG_KEYS.get(operation, set()):
            if key in args and not _is_strict_int(args[key]):
                raise ScenarioValidationError(
                    f"{key} must be an integer (bool rejected)",
                    path,
                )

        # Validate button types: must be 1/2/3 (left/middle/right). Optional —
        # when absent the xdotool default (1 = left) is used.
        for key in BUTTON_ARG_KEYS.get(operation, set()):
            if key in args:
                val = args[key]
                if not _is_strict_int(val) or val not in (1, 2, 3):
                    raise ScenarioValidationError(
                        f"{key} must be 1 (left), 2 (middle), or 3 (right); got {val}",
                        path,
                    )

        # Validate string types
        for key in STR_ARG_KEYS.get(operation, set()):
            if key in args and not isinstance(args[key], str):
                raise ScenarioValidationError(
                    f"{key} must be a string",
                    path,
                )

        # Validate list types
        for key in LIST_ARG_KEYS.get(operation, set()):
            if key in args:
                if not isinstance(args[key], list):
                    raise ScenarioValidationError(
                        f"{key} must be a JSON array",
                        path,
                    )
                if not _is_json_value(args[key]):
                    raise ScenarioValidationError(
                        f"{key} must contain only JSON values",
                        path,
                    )

        # Validate dict types
        for key in DICT_ARG_KEYS.get(operation, set()):
            if key in args:
                if not isinstance(args[key], dict):
                    raise ScenarioValidationError(
                        f"{key} must be a JSON object",
                        path,
                    )
                if not _is_json_value(args[key]):
                    raise ScenarioValidationError(
                        f"{key} must contain only JSON values",
                        path,
                    )

        # Validate VERIFY expected can be any JSON value
        if operation == Operation.VERIFY and "expected" in args:
            if not _is_json_value(args["expected"]):
                raise ScenarioValidationError(
                    "expected must be a JSON value (string, number, boolean, null, array, or object)",
                    path,
                )

        # WINDOW_WAIT state allowlist — reject typos and unknown states
        if operation == Operation.WINDOW_WAIT and "state" in args:
            state_val = args["state"]
            if state_val not in WINDOW_WAIT_ALLOWED_STATES:
                raise ScenarioValidationError(
                    f"WINDOW_WAIT state must be one of "
                    f"{sorted(WINDOW_WAIT_ALLOWED_STATES)}; got '{state_val}'",
                    path,
                )

        # SCROLL direction allowlist — maps to xdotool buttons 4/5/6/7
        if operation == Operation.SCROLL and "direction" in args:
            dir_val = args["direction"]
            if dir_val not in SCROLL_ALLOWED_DIRECTIONS:
                raise ScenarioValidationError(
                    f"SCROLL direction must be one of "
                    f"{sorted(SCROLL_ALLOWED_DIRECTIONS)}; got '{dir_val}'",
                    path,
                )

        # Reject non-JSON values in any argument
        for key, val in args.items():
            if not _is_json_value(val):
                raise ScenarioValidationError(
                    f"{key} must be a JSON value (str, int, float, bool, null, list, or dict)",
                    path,
                )

    @staticmethod
    def _check_verifier(verifier: Any, step_id: str) -> None:
        path = f"steps[{step_id}].verifier"
        if not isinstance(verifier, dict):
            raise ScenarioValidationError("verifier must be an object", path)
        if len(verifier) == 0:
            raise ScenarioValidationError("verifier must not be empty", path)
        known = {"predicate", "expected"}
        unknown = set(verifier.keys()) - known
        if unknown:
            raise ScenarioValidationError(
                f"unknown verifier key(s): {', '.join(sorted(unknown))}",
                path,
            )
        if "predicate" not in verifier:
            raise ScenarioValidationError(
                "missing required verifier key: predicate",
                path,
            )
        if "expected" not in verifier:
            raise ScenarioValidationError(
                "missing required verifier key: expected",
                path,
            )
        predicate = verifier["predicate"]
        if not isinstance(predicate, str) or not predicate:
            raise ScenarioValidationError(
                "verifier.predicate must be a non-empty string",
                path,
            )
        if predicate not in SUPPORTED_PREDICATES:
            raise ScenarioValidationError(
                f"verifier.predicate '{predicate}' is not supported; "
                f"allowed: {sorted(SUPPORTED_PREDICATES)}",
                path,
            )
        if not _is_json_value(verifier["expected"]):
            raise ScenarioValidationError(
                "verifier.expected must be a JSON value",
                path,
            )

    @staticmethod
    def _check_rollback(rollback: Any, step_id: str) -> None:
        path = f"steps[{step_id}].rollback"
        if not isinstance(rollback, dict):
            raise ScenarioValidationError("rollback must be an object", path)
        # rollback must have exactly operation and arguments (no other keys)
        required = {"operation", "arguments"}
        unknown = set(rollback.keys()) - required
        if unknown:
            raise ScenarioValidationError(
                f"unknown rollback field(s): {', '.join(sorted(unknown))}",
                path,
            )
        missing = required - set(rollback.keys())
        if missing:
            raise ScenarioValidationError(
                f"missing required rollback field(s): {', '.join(sorted(missing))}",
                path,
            )
        op_str = rollback["operation"]
        if not isinstance(op_str, str) or op_str not in ALLOWED_OPERATIONS:
            raise ScenarioValidationError(
                f"rollback.operation must be one of {', '.join(sorted(ALLOWED_OPERATIONS))}",
                path,
            )
        operation = Operation(op_str)
        args = rollback["arguments"]
        if not isinstance(args, dict):
            raise ScenarioValidationError("rollback.arguments must be an object", path)
        # Validate rollback arguments using the same operation validation
        Step._check_arguments(args, operation, f"{step_id}.rollback")


@dataclass(frozen=True)
class Scenario:
    version: str
    task_id: str
    session_id: str
    pid: int
    display: str
    cellview: CellView
    steps: Tuple[Step, ...]

    @staticmethod
    def _check_version(ver: Any) -> None:
        if not isinstance(ver, str):
            raise ScenarioValidationError("version must be a string", "version")
        if ver != SUPPORTED_VERSION:
            raise ScenarioValidationError(
                f"version must be exactly \"{SUPPORTED_VERSION}\"; got {ver!r}",
                "version",
            )

    @staticmethod
    def _check_task_id(tid: Any) -> None:
        if not isinstance(tid, str) or not tid:
            raise ScenarioValidationError(
                "task_id must be a non-empty string", "task_id"
            )

    @staticmethod
    def _check_session_id(sid: Any) -> None:
        if not isinstance(sid, str) or not sid:
            raise ScenarioValidationError(
                "session_id must be a non-empty string", "session_id"
            )

    @staticmethod
    def _check_pid(pid: Any) -> None:
        if not _is_strict_int(pid) or pid <= 0:
            raise ScenarioValidationError(
                "pid must be a positive integer", "pid"
            )

    @staticmethod
    def _check_display(display: Any) -> None:
        if not isinstance(display, str):
            raise ScenarioValidationError(
                "display must be a string", "display"
            )
        if not re.match(r"^:[0-9]+(\.[0-9]+)?$", display):
            raise ScenarioValidationError(
                "display must match :N or :N.M format", "display"
            )

    @classmethod
    def from_dict(cls, data: Any) -> "Scenario":
        """Parse and validate a scenario from a dict."""
        if not isinstance(data, dict):
            raise ScenarioValidationError("root must be an object")

        known_top = {"version", "task_id", "session_id", "pid", "display", "cellview", "steps"}
        unknown = set(data.keys()) - known_top
        if unknown:
            raise ScenarioValidationError(
                f"unknown field(s): {', '.join(sorted(unknown))}", "root"
            )

        for field_name in known_top:
            if field_name not in data:
                raise ScenarioValidationError(
                    f"missing required field: {field_name}", field_name
                )

        cls._check_version(data["version"])
        cls._check_task_id(data["task_id"])
        cls._check_session_id(data["session_id"])
        cls._check_pid(data["pid"])
        cls._check_display(data["display"])

        cv_data = data["cellview"]
        if not isinstance(cv_data, dict):
            raise ScenarioValidationError("cellview must be an object", "cellview")
        cv_known = {"lib", "cell", "view"}
        cv_unknown = set(cv_data.keys()) - cv_known
        if cv_unknown:
            raise ScenarioValidationError(
                f"unknown field(s): {', '.join(sorted(cv_unknown))}", "cellview"
            )
        for f in cv_known:
            if f not in cv_data or not isinstance(cv_data[f], str) or not cv_data[f]:
                raise ScenarioValidationError(
                    f"cellview.{f} must be a non-empty string", f"cellview.{f}"
                )
        cellview = CellView(lib=cv_data["lib"], cell=cv_data["cell"], view=cv_data["view"])

        steps_data = data["steps"]
        if not isinstance(steps_data, list):
            raise ScenarioValidationError("steps must be a list", "steps")
        if len(steps_data) == 0:
            raise ScenarioValidationError("steps must not be empty", "steps")

        steps: List[Step] = []
        step_ids_seen = set()
        for i, step_data in enumerate(steps_data):
            path_prefix = f"steps[{i}]"
            if not isinstance(step_data, dict):
                raise ScenarioValidationError("step must be an object", f"{path_prefix}")

            known_step = {"id", "operation", "arguments", "verifier", "timeout_seconds", "max_retries", "rollback"}
            step_unknown = set(step_data.keys()) - known_step
            if step_unknown:
                raise ScenarioValidationError(
                    f"unknown field(s): {', '.join(sorted(step_unknown))}", f"{path_prefix}"
                )

            for f in ["id", "operation", "arguments", "verifier", "timeout_seconds", "max_retries"]:
                if f not in step_data:
                    raise ScenarioValidationError(
                        f"missing required field: {f}", f"{path_prefix}.{f}"
                    )

            step_id = step_data["id"]
            if not isinstance(step_id, str) or not step_id:
                raise ScenarioValidationError(
                    "step id must be a non-empty string", f"{path_prefix}.id"
                )
            if step_id in step_ids_seen:
                raise ScenarioValidationError(
                    f"duplicate step id: {step_id}", f"{path_prefix}.id"
                )
            step_ids_seen.add(step_id)

            op_str = step_data["operation"]
            if not isinstance(op_str, str):
                raise ScenarioValidationError(
                    "operation must be a string", f"{path_prefix}.operation"
                )
            if op_str not in ALLOWED_OPERATIONS:
                raise ScenarioValidationError(
                    f"unknown operation: {op_str}; allowed: {', '.join(sorted(ALLOWED_OPERATIONS))}",
                    f"{path_prefix}.operation"
                )
            operation = Operation(op_str)

            args = step_data.get("arguments")
            if not isinstance(args, dict):
                raise ScenarioValidationError(
                    "arguments must be an object", f"{path_prefix}.arguments"
                )
            Step._check_arguments(args, operation, step_id)

            verifier = step_data.get("verifier")
            Step._check_verifier(verifier, step_id)

            timeout = step_data.get("timeout_seconds")
            if not _is_strict_int(timeout):
                raise ScenarioValidationError(
                    "timeout_seconds must be an integer", f"{path_prefix}.timeout_seconds"
                )
            if not (MIN_TIMEOUT <= timeout <= MAX_TIMEOUT):
                raise ScenarioValidationError(
                    f"timeout_seconds must be {MIN_TIMEOUT}-{MAX_TIMEOUT}",
                    f"{path_prefix}.timeout_seconds"
                )

            retries = step_data.get("max_retries")
            if not _is_strict_int(retries):
                raise ScenarioValidationError(
                    "max_retries must be an integer", f"{path_prefix}.max_retries"
                )
            if not (MIN_RETRIES <= retries <= MAX_RETRIES):
                raise ScenarioValidationError(
                    f"max_retries must be {MIN_RETRIES}-{MAX_RETRIES}",
                    f"{path_prefix}.max_retries"
                )

            rollback = step_data.get("rollback")
            if rollback is not None:
                Step._check_rollback(rollback, step_id)

            # Deep-freeze the parsed data
            frozen_args = _freeze_json(args)
            frozen_verifier = _freeze_json(verifier)
            frozen_rollback = _freeze_json(rollback) if rollback is not None else None

            step = Step(
                id=step_id,
                operation=operation,
                arguments=frozen_args,
                verifier=frozen_verifier,
                timeout_seconds=timeout,
                max_retries=retries,
                rollback=frozen_rollback,
            )
            steps.append(step)

        return cls(
            version=data["version"],
            task_id=data["task_id"],
            session_id=data["session_id"],
            pid=data["pid"],
            display=data["display"],
            cellview=cellview,
            steps=tuple(steps),
        )

    @classmethod
    def load(cls, path: Path) -> "Scenario":
        import json
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return cls.from_dict(data)