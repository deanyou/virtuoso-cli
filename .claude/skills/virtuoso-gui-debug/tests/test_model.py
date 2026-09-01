"""Tests for vgui_runner.model - strict scenario parsing."""
import unittest
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))

from types import MappingProxyType

from vgui_runner.model import (
    Scenario,
    ScenarioValidationError,
    Operation,
    CellView,
    _is_strict_int,
)


VALID_SCENARIO = {
    "version": "1.0",
    "task_id": "test-task-001",
    "session_id": "sess-abc123",
    "pid": 12345,
    "display": ":0",
    "cellview": {"lib": "myLib", "cell": "myCell", "view": "schematic"},
    "steps": [
        {
            "id": "step1",
            "operation": "VCLI_LOAD",
            "arguments": {"command": "myCommand"},
            "verifier": {"predicate": "window_exists", "expected": True},
            "timeout_seconds": 30,
            "max_retries": 1,
        },
        {
            "id": "step2",
            "operation": "WINDOW_WAIT",
            "arguments": {"window_title": "Virtuoso", "state": "visible"},
            "verifier": {"predicate": "state_matches", "expected": "visible"},
            "timeout_seconds": 60,
            "max_retries": 0,
        },
    ],
}


class TestIsStrictInt(unittest.TestCase):
    def test_int_is_strict_int(self):
        self.assertTrue(_is_strict_int(42))

    def test_bool_is_not_strict_int(self):
        self.assertFalse(_is_strict_int(True))
        self.assertFalse(_is_strict_int(False))

    def test_float_is_not_strict_int(self):
        self.assertFalse(_is_strict_int(3.14))

    def test_str_is_not_strict_int(self):
        self.assertFalse(_is_strict_int("42"))


class TestCellView(unittest.TestCase):
    def test_cellview_str(self):
        cv = CellView(lib="myLib", cell="myCell", view="schematic")
        self.assertEqual(str(cv), "myLib/myCell/schematic")


class TestOperationEnum(unittest.TestCase):
    def test_allowed_operations(self):
        self.assertEqual(Operation.VCLI_LOAD.value, "VCLI_LOAD")
        self.assertEqual(Operation.VCLI_CALL.value, "VCLI_CALL")
        self.assertEqual(Operation.WINDOW_WAIT.value, "WINDOW_WAIT")
        self.assertEqual(Operation.WINDOW_ACTIVATE.value, "WINDOW_ACTIVATE")
        self.assertEqual(Operation.KEY.value, "KEY")
        self.assertEqual(Operation.TYPE.value, "TYPE")
        self.assertEqual(Operation.CLICK_REL.value, "CLICK_REL")
        self.assertEqual(Operation.DRAG_REL.value, "DRAG_REL")
        self.assertEqual(Operation.SCREENSHOT.value, "SCREENSHOT")
        self.assertEqual(Operation.VERIFY.value, "VERIFY")
        self.assertEqual(Operation.RECOVER.value, "RECOVER")


class TestValidScenario(unittest.TestCase):
    def test_valid_scenario_parses(self):
        scenario = Scenario.from_dict(VALID_SCENARIO)
        self.assertEqual(scenario.version, "1.0")
        self.assertEqual(scenario.task_id, "test-task-001")
        self.assertEqual(scenario.session_id, "sess-abc123")
        self.assertEqual(scenario.pid, 12345)
        self.assertEqual(scenario.display, ":0")
        self.assertEqual(scenario.cellview.lib, "myLib")
        self.assertEqual(scenario.cellview.cell, "myCell")
        self.assertEqual(scenario.cellview.view, "schematic")
        self.assertEqual(len(scenario.steps), 2)
        self.assertEqual(scenario.steps[0].id, "step1")
        self.assertEqual(scenario.steps[0].operation, Operation.VCLI_LOAD)
        self.assertEqual(scenario.steps[1].id, "step2")
        self.assertEqual(scenario.steps[1].operation, Operation.WINDOW_WAIT)


class TestNonObjectRoot(unittest.TestCase):
    def test_null_root_rejected(self):
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(None)
        self.assertIn("root must be an object", str(ctx.exception))

    def test_list_root_rejected(self):
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict([1, 2, 3])
        self.assertIn("root must be an object", str(ctx.exception))

    def test_string_root_rejected(self):
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict("invalid")
        self.assertIn("root must be an object", str(ctx.exception))

    def test_number_root_rejected(self):
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(42)
        self.assertIn("root must be an object", str(ctx.exception))


class TestUnknownTopLevelField(unittest.TestCase):
    def test_unknown_top_level_field_rejected(self):
        data = dict(VALID_SCENARIO)
        data["unknown_field"] = "value"
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("unknown_field", str(ctx.exception))
        self.assertEqual(ctx.exception.path, "root")


class TestUnknownStepField(unittest.TestCase):
    def test_unknown_step_field_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["unknown_step_field"] = "value"
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("unknown_step_field", str(ctx.exception))


class TestUnknownOperation(unittest.TestCase):
    def test_unknown_operation_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["operation"] = "UNKNOWN_OP"
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("UNKNOWN_OP", str(ctx.exception))
        self.assertIn("unknown operation", str(ctx.exception))


class TestDuplicateStepId(unittest.TestCase):
    def test_duplicate_step_id_rejected(self):
        data = dict(VALID_SCENARIO)
        step = dict(VALID_SCENARIO["steps"][0])
        data["steps"] = [step, dict(step)]
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("duplicate", str(ctx.exception).lower())


class TestMissingVerifier(unittest.TestCase):
    def test_missing_verifier_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        del data["steps"][0]["verifier"]
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("verifier", str(ctx.exception))


class TestInvalidSessionId(unittest.TestCase):
    def test_empty_session_id_rejected(self):
        data = dict(VALID_SCENARIO)
        data["session_id"] = ""
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("session_id", str(ctx.exception))

    def test_missing_session_id_rejected(self):
        data = dict(VALID_SCENARIO)
        del data["session_id"]
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("session_id", str(ctx.exception))


class TestInvalidPid(unittest.TestCase):
    def test_zero_pid_rejected(self):
        data = dict(VALID_SCENARIO)
        data["pid"] = 0
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("pid", str(ctx.exception))

    def test_negative_pid_rejected(self):
        data = dict(VALID_SCENARIO)
        data["pid"] = -1
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("pid", str(ctx.exception))

    def test_bool_pid_rejected(self):
        data = dict(VALID_SCENARIO)
        data["pid"] = True
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("pid", str(ctx.exception))


class TestInvalidDisplay(unittest.TestCase):
    def test_invalid_display_format_rejected(self):
        data = dict(VALID_SCENARIO)
        data["display"] = "invalid"
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("display", str(ctx.exception))


class TestEmptyCellViewComponent(unittest.TestCase):
    def test_empty_cellview_lib_rejected(self):
        data = dict(VALID_SCENARIO)
        data["cellview"] = {"lib": "", "cell": "myCell", "view": "schematic"}
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("cellview.lib", str(ctx.exception))


class TestVersionValidation(unittest.TestCase):
    def test_empty_version_rejected(self):
        data = dict(VALID_SCENARIO)
        data["version"] = ""
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("version", str(ctx.exception))

    def test_number_version_rejected(self):
        data = dict(VALID_SCENARIO)
        data["version"] = 1.0
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("version", str(ctx.exception))


class TestTaskIdValidation(unittest.TestCase):
    def test_empty_task_id_rejected(self):
        data = dict(VALID_SCENARIO)
        data["task_id"] = ""
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("task_id", str(ctx.exception))

    def test_number_task_id_rejected(self):
        data = dict(VALID_SCENARIO)
        data["task_id"] = 123
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("task_id", str(ctx.exception))


class TestTimeoutBounds(unittest.TestCase):
    def test_timeout_too_low_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["timeout_seconds"] = 0
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("timeout", str(ctx.exception))

    def test_timeout_too_high_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["timeout_seconds"] = 301
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("timeout", str(ctx.exception))

    def test_bool_timeout_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["timeout_seconds"] = True
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("timeout_seconds must be an integer", str(ctx.exception))


class TestRetryBounds(unittest.TestCase):
    def test_retry_too_low_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["max_retries"] = -1
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("max_retries", str(ctx.exception))

    def test_retry_too_high_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["max_retries"] = 2
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("max_retries", str(ctx.exception))

    def test_bool_retries_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["max_retries"] = True
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("max_retries must be an integer", str(ctx.exception))


class TestOperationArguments(unittest.TestCase):
    def test_vcli_load_missing_command(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["arguments"] = {}
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("command", str(ctx.exception))

    def test_vcli_call_missing_function(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["id"] = "step2"
        data["steps"][0]["operation"] = "VCLI_CALL"
        data["steps"][0]["arguments"] = {"args": []}
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("function", str(ctx.exception))

    def test_click_rel_missing_x(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["id"] = "step2"
        data["steps"][0]["operation"] = "CLICK_REL"
        data["steps"][0]["arguments"] = {"y": 100, "button": 1}
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("x", str(ctx.exception))

    def test_click_rel_bool_coordinate_rejected(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["id"] = "step2"
        data["steps"][0]["operation"] = "CLICK_REL"
        data["steps"][0]["arguments"] = {"x": True, "y": 100, "button": 1}
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("integer", str(ctx.exception).lower())


class TestScenarioRoundTrip(unittest.TestCase):
    def test_arguments_is_mapping_proxy(self):
        scenario = Scenario.from_dict(VALID_SCENARIO)
        step = scenario.steps[0]
        self.assertIsInstance(step.arguments, MappingProxyType)

    def test_verifier_is_mapping_proxy(self):
        scenario = Scenario.from_dict(VALID_SCENARIO)
        step = scenario.steps[0]
        self.assertIsInstance(step.verifier, MappingProxyType)

    def test_roundtrip_serialization(self):
        scenario = Scenario.from_dict(VALID_SCENARIO)
        self.assertEqual(scenario.version, "1.0")
        self.assertEqual(scenario.task_id, "test-task-001")


class TestStrictVersion(unittest.TestCase):
    def test_version_1_0_accepted(self):
        data = dict(VALID_SCENARIO)
        data["version"] = "1.0"
        scenario = Scenario.from_dict(data)
        self.assertEqual(scenario.version, "1.0")

    def test_version_2_0_rejected(self):
        data = dict(VALID_SCENARIO)
        data["version"] = "2.0"
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("version", str(ctx.exception))
        self.assertIn("1.0", str(ctx.exception))

    def test_version_1_rejected(self):
        data = dict(VALID_SCENARIO)
        data["version"] = "1"
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("version", str(ctx.exception))


class TestOperationArgumentTypes(unittest.TestCase):
    def _make_step(self, **overrides):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        for k, v in overrides.items():
            data["steps"][0][k] = v
        return data

    def test_vcli_load_command_must_be_string(self):
        data = self._make_step(operation="VCLI_LOAD", arguments={"command": 123})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("command", str(ctx.exception))
        self.assertIn("string", str(ctx.exception).lower())

    def test_vcli_call_function_must_be_string(self):
        data = self._make_step(operation="VCLI_CALL",
                               arguments={"function": 42, "args": []})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("function", str(ctx.exception))

    def test_vcli_call_args_must_be_list(self):
        data = self._make_step(operation="VCLI_CALL",
                               arguments={"function": "foo", "args": "notalist"})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("args", str(ctx.exception))

    def test_vcli_call_kwargs_must_be_dict(self):
        data = self._make_step(operation="VCLI_CALL",
                               arguments={"function": "foo", "args": [], "kwargs": "notadict"})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("kwargs", str(ctx.exception))

    def test_window_wait_title_must_be_string(self):
        data = self._make_step(operation="WINDOW_WAIT",
                               arguments={"window_title": 42, "state": "visible"})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("window_title", str(ctx.exception))

    def test_window_activate_title_must_be_string(self):
        data = self._make_step(operation="WINDOW_ACTIVATE",
                               arguments={"window_title": []})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("window_title", str(ctx.exception))

    def test_key_keys_must_be_string(self):
        data = self._make_step(operation="KEY", arguments={"keys": 99})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("keys", str(ctx.exception))

    def test_type_text_must_be_string(self):
        data = self._make_step(operation="TYPE", arguments={"text": 1})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("text", str(ctx.exception))

    def test_click_rel_bool_button_rejected(self):
        data = self._make_step(operation="CLICK_REL",
                               arguments={"x": 10, "y": 20, "button": True})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("button", str(ctx.exception))

    def test_click_rel_negative_button_rejected(self):
        data = self._make_step(operation="CLICK_REL",
                               arguments={"x": 10, "y": 20, "button": -1})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("button", str(ctx.exception))

    def test_drag_rel_bool_x1_rejected(self):
        data = self._make_step(operation="DRAG_REL",
                               arguments={"x1": True, "y1": 0, "x2": 0, "y2": 0, "button": 1})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("x1", str(ctx.exception))

    def test_screenshot_path_must_be_string(self):
        data = self._make_step(operation="SCREENSHOT", arguments={"path": 7})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("path", str(ctx.exception))

    def test_screenshot_no_path_accepted(self):
        data = self._make_step(operation="SCREENSHOT", arguments={})
        scenario = Scenario.from_dict(data)
        self.assertEqual(len(scenario.steps), 1)

    def test_verify_predicate_must_be_string(self):
        data = self._make_step(operation="VERIFY",
                               arguments={"predicate": 1, "expected": True})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("predicate", str(ctx.exception))

    def test_verify_expected_any_json_value(self):
        # Strings, ints, floats, bools, null, lists, dicts should all be accepted
        for expected in ["string", 42, 3.14, True, False, None, [1, 2], {"k": "v"}]:
            data = self._make_step(operation="VERIFY",
                                   arguments={"predicate": "p", "expected": expected})
            scenario = Scenario.from_dict(data)
            frozen = scenario.steps[0].arguments["expected"]
            if isinstance(expected, list):
                self.assertEqual(frozen, tuple(expected))
            elif isinstance(expected, dict):
                self.assertEqual(dict(frozen), expected)
            else:
                self.assertEqual(frozen, expected)

    def test_recover_action_must_be_string(self):
        data = self._make_step(operation="RECOVER",
                               arguments={"action": 1, "target": "x"})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("action", str(ctx.exception))

    def test_recover_target_must_be_string(self):
        data = self._make_step(operation="RECOVER",
                               arguments={"action": "x", "target": 1})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("target", str(ctx.exception))


class TestVerifierValidation(unittest.TestCase):
    def _make_step(self, **overrides):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        for k, v in overrides.items():
            data["steps"][0][k] = v
        return data

    def test_empty_verifier_rejected(self):
        data = self._make_step(verifier={})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("verifier", str(ctx.exception))
        self.assertIn("empty", str(ctx.exception).lower())

    def test_verifier_missing_predicate_rejected(self):
        data = self._make_step(verifier={"expected": True})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("verifier", str(ctx.exception))
        self.assertIn("predicate", str(ctx.exception))

    def test_verifier_missing_expected_rejected(self):
        data = self._make_step(verifier={"predicate": "p"})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("verifier", str(ctx.exception))
        self.assertIn("expected", str(ctx.exception))

    def test_verifier_unknown_key_rejected(self):
        data = self._make_step(verifier={"predicate": "p", "expected": True, "extra": "x"})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("verifier", str(ctx.exception))
        self.assertIn("extra", str(ctx.exception))

    def test_verifier_predicate_must_be_string(self):
        data = self._make_step(verifier={"predicate": 1, "expected": True})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("verifier", str(ctx.exception))
        self.assertIn("predicate", str(ctx.exception))

    def test_verifier_predicate_non_empty(self):
        data = self._make_step(verifier={"predicate": "", "expected": True})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("verifier", str(ctx.exception))

    def test_verifier_not_object_rejected(self):
        data = self._make_step(verifier=["predicate", "expected"])
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("verifier", str(ctx.exception))


class TestRollbackValidation(unittest.TestCase):
    def _make_step(self, **overrides):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        for k, v in overrides.items():
            data["steps"][0][k] = v
        return data

    def test_rollback_valid(self):
        data = self._make_step(rollback={
            "operation": "KEY",
            "arguments": {"keys": "Escape"},
        })
        scenario = Scenario.from_dict(data)
        self.assertIsNotNone(scenario.steps[0].rollback)
        self.assertEqual(scenario.steps[0].rollback["operation"], "KEY")

    def test_rollback_missing_operation_rejected(self):
        data = self._make_step(rollback={"arguments": {"keys": "Escape"}})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("rollback", str(ctx.exception))
        self.assertIn("operation", str(ctx.exception))

    def test_rollback_missing_arguments_rejected(self):
        data = self._make_step(rollback={"operation": "KEY"})
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("rollback", str(ctx.exception))
        self.assertIn("arguments", str(ctx.exception))

    def test_rollback_unknown_operation_rejected(self):
        data = self._make_step(rollback={
            "operation": "BOGUS_OP",
            "arguments": {},
        })
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("rollback", str(ctx.exception))

    def test_rollback_invalid_arguments_rejected(self):
        data = self._make_step(rollback={
            "operation": "KEY",
            "arguments": {"keys": 42},
        })
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("rollback", str(ctx.exception))
        self.assertIn("keys", str(ctx.exception))

    def test_rollback_not_dict_rejected(self):
        data = self._make_step(rollback="notadict")
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("rollback", str(ctx.exception))

    def test_rollback_unknown_field_rejected(self):
        data = self._make_step(rollback={
            "operation": "KEY",
            "arguments": {"keys": "Escape"},
            "extra": "x",
        })
        with self.assertRaises(ScenarioValidationError) as ctx:
            Scenario.from_dict(data)
        self.assertIn("rollback", str(ctx.exception))


class TestDeepImmutability(unittest.TestCase):
    def test_arguments_are_mapping_proxy(self):
        scenario = Scenario.from_dict(VALID_SCENARIO)
        step = scenario.steps[0]
        self.assertIsInstance(step.arguments, MappingProxyType)

    def test_verifier_is_mapping_proxy(self):
        scenario = Scenario.from_dict(VALID_SCENARIO)
        step = scenario.steps[0]
        self.assertIsInstance(step.verifier, MappingProxyType)

    def test_nested_list_frozen_to_tuple(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["operation"] = "VCLI_CALL"
        data["steps"][0]["arguments"] = {
            "function": "foo",
            "args": [{"a": 1}, [1, 2], "3.3"],
        }
        scenario = Scenario.from_dict(data)
        args = scenario.steps[0].arguments
        self.assertIsInstance(args["args"], tuple)
        # Inner dict is MappingProxyType
        self.assertIsInstance(args["args"][0], MappingProxyType)
        # Inner list is tuple
        self.assertIsInstance(args["args"][1], tuple)
        # Inner str is preserved
        self.assertEqual(args["args"][2], "3.3")

    def test_nested_dict_frozen_to_mapping_proxy(self):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        data["steps"][0]["rollback"] = {
            "operation": "VCLI_CALL",
            "arguments": {
                "function": "foo",
                "args": [],
                "kwargs": {"nested": {"a": [2, 3]}},
            },
        }
        scenario = Scenario.from_dict(data)
        rb = scenario.steps[0].rollback
        self.assertIsInstance(rb, MappingProxyType)
        self.assertIsInstance(rb["arguments"]["kwargs"], MappingProxyType)
        self.assertIsInstance(rb["arguments"]["kwargs"]["nested"], MappingProxyType)
        self.assertIsInstance(rb["arguments"]["kwargs"]["nested"]["a"], tuple)

    def test_cannot_mutate_arguments(self):
        scenario = Scenario.from_dict(VALID_SCENARIO)
        step = scenario.steps[0]
        with self.assertRaises(TypeError):
            step.arguments["command"] = "other"  # type: ignore

    def test_cannot_mutate_verifier(self):
        scenario = Scenario.from_dict(VALID_SCENARIO)
        step = scenario.steps[0]
        with self.assertRaises(TypeError):
            step.verifier["new"] = "value"  # type: ignore


class TestNonJsonValues(unittest.TestCase):
    def _make_step(self, **overrides):
        data = dict(VALID_SCENARIO)
        data["steps"] = [dict(VALID_SCENARIO["steps"][0])]
        for k, v in overrides.items():
            data["steps"][0][k] = v
        return data

    def test_verifier_with_set_rejected(self):
        data = self._make_step(verifier={"predicate": "p", "expected": {1, 2}})
        with self.assertRaises(ScenarioValidationError):
            Scenario.from_dict(data)

    def test_verifier_with_tuple_rejected(self):
        # Tuples are valid Python but not valid JSON (JSON has arrays)
        # Actually JSON arrays are lists; tuples are also non-JSON in JSON spec terms.
        # Our _is_json_value accepts lists, not tuples. Tuples would be a non-JSON value.
        data = self._make_step(verifier={"predicate": "p", "expected": (1, 2)})
        with self.assertRaises(ScenarioValidationError):
            Scenario.from_dict(data)


if __name__ == "__main__":
    unittest.main()
