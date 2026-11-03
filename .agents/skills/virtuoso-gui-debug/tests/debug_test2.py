import sys, json, tempfile
from pathlib import Path
sys.path.insert(0, '.')
from vgui_runner.model import Scenario
from vgui_runner.engine import Runner, FakeExecutor, StepOutcome
from vgui_runner.verifier_result import VerifierResult, VerifyStatus

class FailingVerify(FakeExecutor):
    def verify(self, step, attempt):
        return VerifierResult(predicate='x', status=VerifyStatus.FAILED,
                              reason_code='timeout', step_id=step.id, attempt=attempt)

scenario = Scenario.from_dict({
    'version': '1.0', 'task_id': 't', 'session_id': 's', 'pid': 1, 'display': ':0',
    'cellview': {'lib': 'L', 'cell': 'C', 'view': 'layout'},
    'steps': [{'id': 's1', 'operation': 'SCREENSHOT', 'arguments': {},
               'verifier': {'predicate': 'window_exists', 'expected': True},
               'timeout_seconds': 5, 'max_retries': 0}],
})
tmp = Path(tempfile.mkdtemp())
runner = Runner(FailingVerify({}))
s = runner.run(scenario, tmp / 'run')
print(f'passed={s.passed} error={s.error_code} phase={s.phase}')
events = [json.loads(l) for l in (tmp/'run'/'agent-actions.jsonl').read_text().strip().split('\n') if l]
for e in events:
    d = e.get('details') or {}
    print(f'  {e["seq"]:2d} {e["state"]:10s} att={e["attempt"]} out={e["outcome"]} det={d}')
