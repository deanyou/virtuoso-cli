"""vgui_runner.planner - Template-based action graph planner.

Generates constrained Scenarios from pre-approved task templates.
Does NOT generate arbitrary GUI actions. Every template has:
- fixed step sequence
- declared risk class
- built-in verifier
- recovery policy hint

Usage:
    planner = Planner()
    scenario = planner.plan("screenshot_window", window_title="CIW")
    runner.run(scenario, output_dir)
"""

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from .model import Scenario, Step, Operation
from .router import RiskClass


@dataclass(frozen=True)
class TemplateStep:
    operation: Operation
    arguments: Dict[str, Any]
    verifier: Optional[Dict[str, Any]] = None
    timeout_seconds: int = 10
    max_retries: int = 0
    rollback: Optional[Dict[str, Any]] = None


@dataclass(frozen=True)
class TaskTemplate:
    name: str
    description: str
    risk_class: RiskClass
    steps: tuple  # tuple[TemplateStep, ...]
    required_params: tuple  # params that must be filled
    optional_params: tuple = ()


# Pre-approved templates. Each is a fixed action graph with known risk.
TEMPLATES: Dict[str, TaskTemplate] = {
    "screenshot_window": TaskTemplate(
        name="screenshot_window",
        description="Take a screenshot of a named window (read-only)",
        risk_class=RiskClass.READ_ONLY,
        required_params=("window_title",),
        optional_params=(),
        steps=(
            TemplateStep(
                operation=Operation.WINDOW_ACTIVATE,
                arguments={"window_title": "{window_title}"},
                verifier={"predicate": "window_exists", "expected": True},
                timeout_seconds=5,
            ),
            TemplateStep(
                operation=Operation.SCREENSHOT,
                arguments={},
                verifier={"predicate": "window_exists", "expected": True},
                timeout_seconds=10,
            ),
        ),
    ),
    "key_press": TaskTemplate(
        name="key_press",
        description="Send a keypress to a window (idempotent)",
        risk_class=RiskClass.IDEMPOTENT_WRITE,
        required_params=("window_title", "key"),
        optional_params=(),
        steps=(
            TemplateStep(
                operation=Operation.WINDOW_ACTIVATE,
                arguments={"window_title": "{window_title}"},
                timeout_seconds=5,
            ),
            TemplateStep(
                operation=Operation.KEY,
                arguments={"key": "{key}"},
                verifier={"predicate": "window_exists", "expected": True},
                timeout_seconds=5,
                max_retries=1,
                rollback={"operation": "KEY", "arguments": {"key": "Escape"}},
            ),
        ),
    ),
    "ciw_command": TaskTemplate(
        name="ciw_command",
        description="Execute a SKILL command in CIW (non-idempotent)",
        risk_class=RiskClass.NON_IDEMPOTENT_WRITE,
        required_params=("command",),
        optional_params=("window_title",),
        steps=(
            TemplateStep(
                operation=Operation.CIW_INPUT,
                arguments={"text": "{command}"},
                verifier={"predicate": "ciw_eval", "expected": True},
                timeout_seconds=15,
                max_retries=0,  # no auto-retry for non-idempotent
            ),
        ),
    ),
    "window_close": TaskTemplate(
        name="window_close",
        description="Close a window (destructive)",
        risk_class=RiskClass.DESTRUCTIVE,
        required_params=("window_id",),
        optional_params=(),
        steps=(
            TemplateStep(
                operation=Operation.CLOSE,
                arguments={"window_id": "{window_id}"},
                verifier={"predicate": "window_exists", "expected": False},
                timeout_seconds=5,
                max_retries=0,
            ),
        ),
    ),
}


class Planner:
    """Template-based scenario planner.

    Generates Scenario objects from pre-approved templates.
    Does NOT generate arbitrary actions — only fills template parameters.
    """

    def __init__(self, templates: Optional[Dict[str, TaskTemplate]] = None):
        self._templates = templates or TEMPLATES

    def available_templates(self) -> List[Dict[str, Any]]:
        """List all available templates with their requirements."""
        return [
            {
                "name": t.name,
                "description": t.description,
                "risk_class": t.risk_class.value,
                "required_params": list(t.required_params),
                "optional_params": list(t.optional_params),
                "steps": len(t.steps),
            }
            for t in self._templates.values()
        ]

    def plan(self, template_name: str, *, task_id: str, session_id: str,
             pid: int, display: str, cellview: Dict[str, str],
             **params: Any) -> Scenario:
        """Build a Scenario from a template + parameter values.

        Raises ValueError if required params missing or template unknown.
        """
        if template_name not in self._templates:
            raise ValueError(
                f"Unknown template '{template_name}'. "
                f"Available: {list(self._templates.keys())}"
            )
        tpl = self._templates[template_name]

        # Validate required params
        missing = [p for p in tpl.required_params if p not in params]
        if missing:
            raise ValueError(
                f"Template '{template_name}' requires params: {missing}"
            )

        # Fill steps
        steps = []
        for i, ts in enumerate(tpl.steps):
            args = self._fill_args(ts.arguments, params)
            step_dict = {
                "id": f"{template_name}-step-{i+1}",
                "operation": ts.operation.value,
                "arguments": args,
                "timeout_seconds": ts.timeout_seconds,
                "max_retries": ts.max_retries,
            }
            if ts.verifier:
                step_dict["verifier"] = self._fill_args(ts.verifier, params)
            if ts.rollback:
                step_dict["rollback"] = self._fill_args(ts.rollback, params)
            steps.append(step_dict)

        scenario_dict = {
            "version": "1.0",
            "task_id": task_id,
            "session_id": session_id,
            "pid": pid,
            "display": display,
            "cellview": cellview,
            "steps": steps,
        }
        return Scenario.from_dict(scenario_dict)

    def _fill_args(self, args: Dict[str, Any], params: Dict[str, Any]) -> Dict[str, Any]:
        """Replace {param} placeholders with actual values."""
        result = {}
        for k, v in args.items():
            if isinstance(v, str) and v.startswith("{") and v.endswith("}"):
                key = v[1:-1]
                if key in params:
                    result[k] = params[key]
                else:
                    result[k] = v  # leave placeholder if not filled
            else:
                result[k] = v
        return result
