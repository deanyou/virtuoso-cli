#!/usr/bin/env python3
"""
vcli_cmd.py — build a vcli invocation as an argv list.

The single source of truth for turning a brief action into the arguments
`vcli` receives.  It returns a **list**, never a shell string: a brief is
data that may carry a cell name, a view name or a design variable copied
from somewhere else, and a value like ``top; rm -rf $HOME`` must reach vcli
as one argument, not as a second command.

`brief.py` and `run.py` both build from here so the two cannot drift into
different quoting rules again — they did once, and the divergence was the
bug.
"""

import shlex
from typing import Any, Dict, List

# Brief action name -> vcli subcommand.
ACTION_MAP: Dict[str, List[str]] = {
    'maestro.open_session': ['maestro', 'open-session'],
    'maestro.run': ['maestro', 'run'],
    'maestro.save': ['maestro', 'save'],
    'maestro.set_var': ['maestro', 'set-var'],
    'maestro.get_var': ['maestro', 'get-var'],
    'maestro.list_sessions': ['maestro', 'list-sessions'],
    'skill.exec': ['skill', 'exec'],
    'spectre.run': ['spectre', 'run'],
    'cell.open': ['cell', 'open'],
    'cell.info': ['cell', 'info'],
}


def build_argv(action: str, args: Dict[str, Any]) -> List[str]:
    """
    Build the argv for one vcli invocation.

    Every value becomes exactly one argv element, whatever it contains.  A
    boolean contributes only its flag when true; a number or a string
    contributes the flag and the value as two elements.

    Args:
        action: Brief action name (e.g. "maestro.open_session")
        args: Action arguments

    Returns:
        The argument list to hand to subprocess, starting with "vcli".
    """
    argv = ['vcli'] + ACTION_MAP.get(action, action.replace('.', ' ').split())

    for key, value in (args or {}).items():
        if isinstance(value, bool):
            if value:
                argv.append(f'--{key}')
        else:
            argv.append(f'--{key}')
            argv.append(str(value))

    return argv


def display(argv: List[str]) -> str:
    """Render an argv for humans — for logs and for the `brief.py cmd` output."""
    return shlex.join(argv)
