#!/usr/bin/python3
"""
run.py — Execute vcli commands from brief

Usage:
    python3 run.py execute <brief.json>
    python3 run.py status <brief_id>
"""

import json
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional


def run_vcli_command(cmd: str, timeout: int = 300) -> Dict[str, Any]:
    """
    Execute vcli command and return result.
    
    Args:
        cmd: vcli command string
        timeout: Command timeout in seconds
    
    Returns:
        Result dict with stdout, stderr, returncode, duration_ms
    """
    start_time = time.time()
    
    try:
        result = subprocess.run(
            cmd,
            shell=True,
            capture_output=True,
            text=True,
            timeout=timeout
        )
        duration_ms = int((time.time() - start_time) * 1000)
        
        return {
            'command': cmd,
            'stdout': result.stdout,
            'stderr': result.stderr,
            'returncode': result.returncode,
            'duration_ms': duration_ms,
            'success': result.returncode == 0
        }
    except subprocess.TimeoutExpired:
        duration_ms = int((time.time() - start_time) * 1000)
        return {
            'command': cmd,
            'stdout': '',
            'stderr': f'Command timed out after {timeout}s',
            'returncode': -1,
            'duration_ms': duration_ms,
            'success': False,
            'error': 'timeout'
        }
    except Exception as e:
        duration_ms = int((time.time() - start_time) * 1000)
        return {
            'command': cmd,
            'stdout': '',
            'stderr': str(e),
            'returncode': -1,
            'duration_ms': duration_ms,
            'success': False,
            'error': 'exception'
        }


def action_to_vcli(action: str, args: Dict[str, Any]) -> str:
    """Convert action to vcli command."""
    action_map = {
        'maestro.open_session': 'maestro open-session',
        'maestro.run': 'maestro run',
        'maestro.save': 'maestro save',
        'maestro.set_var': 'maestro set-var',
        'maestro.get_var': 'maestro get-var',
        'maestro.list_sessions': 'maestro list-sessions',
        'skill.exec': 'skill exec',
        'spectre.run': 'spectre run',
        'cell.open': 'cell open',
        'cell.info': 'cell info',
    }
    
    base = action_map.get(action, action.replace('.', ' '))
    cmd_parts = [f'vcli {base}']
    
    for key, value in args.items():
        if isinstance(value, bool):
            if value:
                cmd_parts.append(f'--{key}')
        elif isinstance(value, (int, float)):
            cmd_parts.append(f'--{key}')
            cmd_parts.append(str(value))
        elif isinstance(value, str):
            if ' ' in value or '"' in value:
                cmd_parts.append(f'--{key}')
                cmd_parts.append(f'"{value}"')
            else:
                cmd_parts.append(f'--{key}')
                cmd_parts.append(value)
    
    return ' '.join(cmd_parts)


def execute_command(command: Dict[str, Any], context: Dict[str, Any]) -> Dict[str, Any]:
    """
    Execute a single command from brief.
    
    Args:
        command: Command dict with 'action' and optional 'args'
        context: Execution context (artifacts from previous commands)
    
    Returns:
        Execution result dict
    """
    action = command.get('action')
    args = command.get('args', {})
    timeout = command.get('timeout', 300)
    
    # Substitute template variables
    args_str = json.dumps(args)
    for key, value in context.items():
        args_str = args_str.replace(f'{{{{{key}}}}}', str(value))
    args = json.loads(args_str)
    
    # Build vcli command
    cmd = action_to_vcli(action, args)
    
    # Handle Maestro session context
    if action == 'maestro.open_session':
        # Expect result to be session name
        pass
    
    print(f"  → {cmd}")
    
    result = run_vcli_command(cmd, timeout=timeout)
    
    # Extract artifacts
    artifacts = {}
    if result['success']:
        if action == 'maestro.open_session':
            # Try to parse session name from output
            try:
                output = json.loads(result['stdout'])
                if 'session' in output:
                    artifacts['maestro_session'] = output['session']
            except:
                pass
        elif action == 'maestro.run':
            artifacts['maestro_session'] = context.get('maestro_session', '')
    
    return {
        'action': action,
        'args': args,
        **result,
        'artifacts': artifacts
    }


def execute_brief(brief: Dict[str, Any]) -> Dict[str, Any]:
    """
    Execute all commands in brief.
    
    Args:
        brief: Execution brief with commands
    
    Returns:
        Updated brief with results
    """
    # Update status
    brief['status'] = 'running'
    brief['started'] = datetime.utcnow().isoformat() + 'Z'
    
    commands = brief.get('requirement', {}).get('commands', [])
    context = {}  # Shared context between commands
    
    print(f"Executing {len(commands)} commands...")
    
    results = []
    for i, cmd in enumerate(commands):
        print(f"\n[{i+1}/{len(commands)}] {cmd.get('action')}")
        
        result = execute_command(cmd, context)
        results.append(result)
        
        # Update context with artifacts
        if result.get('artifacts'):
            context.update(result['artifacts'])
        
        # Check for failure
        if not result.get('success', False):
            print(f"  ✗ Failed: {result.get('stderr', 'Unknown error')}")
            brief['status'] = 'failed'
            brief['failed_at'] = i
            brief['failure_reason'] = result.get('stderr', 'Command failed')
            break
        else:
            print(f"  ✓ Success ({result.get('duration_ms', 0)}ms)")
    
    else:
        # All commands succeeded
        brief['status'] = 'success'
    
    # Store results
    brief['commands'] = results
    brief['artifacts'] = context
    brief['completed'] = datetime.utcnow().isoformat() + 'Z'
    
    return brief


def main():
    """CLI entry point."""
    if len(sys.argv) < 3:
        print("Usage:")
        print("  run.py execute <brief.json>")
        print("  run.py status <brief_id>")
        sys.exit(1)
    
    cmd = sys.argv[1]
    arg = sys.argv[2]
    
    if cmd == 'execute':
        path = Path(arg)
        brief = json.loads(path.read_text())
        brief = execute_brief(brief)
        
        # Save updated brief
        output_path = path.parent / f"{brief['id']}.json"
        output_path.write_text(json.dumps(brief, indent=2))
        print(f"\nBrief saved to: {output_path}")
        
        # Print summary
        print(f"\n=== Summary ===")
        print(f"Status: {brief['status']}")
        print(f"Commands executed: {len(brief.get('commands', []))}")
        if brief.get('artifacts'):
            print(f"Artifacts: {brief['artifacts']}")
    
    elif cmd == 'status':
        # Load brief by ID
        brief_dir = Path.home() / '.cache' / 'virtuoso_bridge' / 'workflow'
        brief_path = list(brief_dir.glob(f"*{arg}*.json"))
        if brief_path:
            brief = json.loads(brief_path[0].read_text())
            print(json.dumps(brief, indent=2))
        else:
            print(f"Brief not found: {arg}")
            sys.exit(1)
    
    else:
        print(f"Unknown command: {cmd}")
        sys.exit(1)


if __name__ == '__main__':
    main()
