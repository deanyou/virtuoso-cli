"""vgui_runner.planner - Template-based action graph planner.

Generates constrained Scenarios from pre-approved task templates.
Does NOT generate arbitrary GUI actions. Every template has:
- fixed step sequence (template_id + template_version for evidence)
- declared risk class
- built-in verifier
- recovery policy hint
- documented side_effects
"""

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple
import uuid

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
    template_id: str
    template_version: str
    description: str
    risk_class: RiskClass
    steps: Tuple[TemplateStep, ...]
    required_params: Tuple[str, ...]
    optional_params: Tuple[str, ...] = ()
    side_effects: Tuple[str, ...] = ()
    postconditions: Tuple[str, ...] = ()


# Pre-approved templates. Conservative defaults:
# - key_press is non_idempotent (Enter/Delete/hotkeys are irreversible)
# - screenshot_window has focus side_effect (not pure read_only)
# - window_close has async wait before verify
TEMPLATES: Dict[str, TaskTemplate] = {
    "screenshot_window": TaskTemplate(
        template_id="screenshot_window",
        template_version="1.0.0",
        description="Take a screenshot of a named window. Activate changes focus.",
        risk_class=RiskClass.IDEMPOTENT_WRITE,  # not pure read_only — focus side effect
        required_params=("window_title",),
        optional_params=(),
        side_effects=("window_focus",),
        postconditions=("window_exists",),
        steps=(
            TemplateStep(
                operation=Operation.WINDOW_ACTIVATE,
                arguments={"window_title": "{window_title}"},
                verifier={"predicate": "window_exists", "expected": True},
                timeout_seconds=5,
                max_retries=1,
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
        template_id="key_press",
        template_version="1.1.0",
        description="Send a keypress. Conservative: non_idempotent by default.",
        risk_class=RiskClass.NON_IDEMPOTENT_WRITE,
        required_params=("window_title", "key"),
        optional_params=("allow_retry",),
        side_effects=("window_focus", "key_event_dispatched"),
        postconditions=("window_exists",),
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
                max_retries=0,  # P1: no auto-retry by default
            ),
        ),
    ),
    "ciw_command": TaskTemplate(
        template_id="ciw_command",
        template_version="1.0.0",
        description="Execute a SKILL command in CIW. Non-idempotent by default.",
        risk_class=RiskClass.NON_IDEMPOTENT_WRITE,
        required_params=("command",),
        optional_params=("window_title",),
        side_effects=("ciw_state_change",),
        postconditions=("ciw_eval",),
        steps=(
            TemplateStep(
                operation=Operation.CIW_INPUT,
                arguments={"text": "{command}"},
                verifier={"predicate": "ciw_eval", "expected": True},
                timeout_seconds=15,
                max_retries=0,
            ),
        ),
    ),
    "window_close": TaskTemplate(
        template_id="window_close",
        template_version="1.1.0",
        description="Close a window. Waits for async close before verify.",
        risk_class=RiskClass.DESTRUCTIVE,
        required_params=("window_id",),
        optional_params=(),
        side_effects=("window_destroyed",),
        postconditions=("window_not_exists",),
        steps=(
            TemplateStep(
                operation=Operation.CLOSE,
                arguments={"window_id": "{window_id}"},
                verifier={"predicate": "window_exists", "expected": True},
                timeout_seconds=5,
                max_retries=0,
            ),
            TemplateStep(
                operation=Operation.WINDOW_WAIT,
                arguments={"window_title": "", "state": "hidden"},
                verifier={"predicate": "window_exists", "expected": False},
                timeout_seconds=10,
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
                "template_id": t.template_id,
                "template_version": t.template_version,
                "description": t.description,
                "risk_class": t.risk_class.value,
                "required_params": list(t.required_params),
                "optional_params": list(t.optional_params),
                "side_effects": list(t.side_effects),
                "postconditions": list(t.postconditions),
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
        plan_id = f"plan-{uuid.uuid4().hex[:8]}"

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
                    result[k] = v
            else:
                result[k] = v
        return result
