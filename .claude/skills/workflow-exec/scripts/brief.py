#!/usr/bin/python3
"""
brief.py — Generate Execution Brief with SHA-256 manifest

Usage:
    python3 brief.py generate <requirement.yaml>
    python3 brief.py verify <brief.json>
"""

import hashlib
import json
import sys
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional

try:
    import yaml
except ImportError:
    yaml = None


def _parse_value(value: str) -> Any:
    """Parse string value to appropriate Python type (NO eval)."""
    if not value:
        return ''
    # Boolean
    if value.lower() == 'true':
        return True
    if value.lower() == 'false':
        return False
    if value.lower() == 'null' or value.lower() == '~':
        return None
    # Integer
    try:
        return int(value)
    except ValueError:
        pass
    # Float (including scientific notation like 10e6)
    try:
        return float(value)
    except ValueError:
        pass
    # String (keep as-is)
    return value


def simple_yaml_parse(s: str) -> Dict[str, Any]:
    """Simple YAML parser for basic requirement format (no PyYAML needed)."""
    if yaml is None:
        # Fallback: parse key: value lines (NO eval for security)
        result = {}
        current_key = None
        current_list = None
        
        for line in s.strip().split('\n'):
            line = line.strip()
            if not line or line.startswith('#'):
                continue
            
            # Check for list item
            if line.startswith('- '):
                if current_list is not None:
                    current_list.append(line[2:].strip())
            elif ':' in line:
                key, _, value = line.partition(':')
                key = key.strip()
                value = value.strip()
                
                if not value:  # Multiline value or empty
                    current_key = key
                    current_list = None
                    result[key] = {}
                else:
                    # Safe type parsing (NO eval)
                    result[key] = _parse_value(value)
        
        return result
    else:
        return yaml.safe_load(s)


def sha256_content(content: str) -> str:
    """Compute SHA-256 hex digest of content."""
    return hashlib.sha256(content.encode()).hexdigest()


def parse_requirement(requirement: str) -> Dict[str, Any]:
    """Parse requirement from YAML string."""
    return simple_yaml_parse(requirement) if yaml is None else yaml.safe_load(requirement)


def generate_brief_id() -> str:
    """Generate unique brief ID."""
    now = datetime.utcnow()
    return f"brief-{now.strftime('%Y%m%d-%H%M%S')}"


def generate_brief(requirement_yaml: str, requirement_path: Optional[str] = None) -> Dict[str, Any]:
    """
    Generate execution brief from requirement YAML.
    
    Args:
        requirement_yaml: YAML content as string
        requirement_path: Optional path to original file (for provenance)
    
    Returns:
        Execution brief dict with SHA-256 hash
    """
    # Parse requirement
    req = parse_requirement(requirement_yaml)
    
    # Generate ID and timestamp
    brief_id = req.get('id') or generate_brief_id()
    created = datetime.utcnow().isoformat() + 'Z'
    
    # Compute SHA-256 of requirement content
    sha256 = sha256_content(requirement_yaml)
    
    # Build brief structure
    brief = {
        'id': brief_id,
        'sha256': sha256,
        'created': created,
        'source': requirement_path,
        'status': 'pending',
        'requirement': req,
        'commands': [],
        'artifacts': {},
        'verification': {
            'passed': None,
            'checks': []
        }
    }
    
    return brief


def verify_brief(brief: Dict[str, Any], requirement_yaml: Optional[str] = None) -> bool:
    """
    Verify brief integrity.
    
    Args:
        brief: Execution brief to verify
        requirement_yaml: Optional requirement YAML to re-verify SHA-256
    
    Returns:
        True if valid, False otherwise
    """
    # Check required fields
    required_fields = ['id', 'sha256', 'created', 'requirement']
    for field in required_fields:
        if field not in brief:
            print(f"ERROR: Missing required field: {field}")
            return False
    
    # Verify SHA-256 if requirement provided
    if requirement_yaml:
        expected_sha256 = sha256_content(requirement_yaml)
        if brief['sha256'] != expected_sha256:
            print(f"ERROR: SHA-256 mismatch!")
            print(f"  Expected: {expected_sha256}")
            print(f"  Got:      {brief['sha256']}")
            return False
        print(f"✓ SHA-256 verified: {brief['sha256'][:16]}...")
    
    print(f"✓ Brief ID: {brief['id']}")
    print(f"✓ Created: {brief['created']}")
    print(f"✓ Status:  {brief['status']}")
    
    # Check status
    status = brief.get('status')
    if status == 'failed':
        print(f"✗ Execution failed: {brief.get('failure_reason', 'Unknown')}")
        return False
    elif status == 'pending':
        print(f"⚠ Brief not executed yet")
    elif status == 'running':
        print(f"⚠ Brief execution in progress")
    
    return True


def command_to_vcli(action: str, args: Dict[str, Any]) -> str:
    """
    Convert action to vcli command.
    
    Args:
        action: Action name (e.g., "maestro.open_session")
        args: Action arguments
    
    Returns:
        vcli command string
    """
    action_map = {
        'maestro.open_session': 'vcli maestro open-session',
        'maestro.run': 'vcli maestro run',
        'maestro.save': 'vcli maestro save',
        'maestro.set_var': 'vcli maestro set-var',
        'maestro.get_var': 'vcli maestro get-var',
        'skill.exec': 'vcli skill exec',
        'spectre.run': 'vcli spectre run',
        'cell.open': 'vcli cell open',
        'cell.info': 'vcli cell info',
    }
    
    if action not in action_map:
        return f"vcli {action.replace('.', ' ')}"
    
    base_cmd = action_map.get(action, f"vcli {action}")
    
    # Build argument string
    arg_parts = []
    for key, value in args.items():
        if isinstance(value, bool):
            if value:
                arg_parts.append(f"--{key}")
        elif isinstance(value, (int, float)):
            arg_parts.append(f"--{key} {value}")
        else:
            arg_parts.append(f"--{key} '{value}'")
    
    if arg_parts:
        return f"{base_cmd} {' '.join(arg_parts)}"
    return base_cmd


def main():
    """CLI entry point."""
    if len(sys.argv) < 3:
        print("Usage:")
        print("  brief.py generate <requirement.yaml>")
        print("  brief.py verify <brief.json>")
        print("  brief.py cmd '<action>' '<args_json>'")
        sys.exit(1)
    
    cmd = sys.argv[1]
    arg = sys.argv[2]
    
    if cmd == 'generate':
        # Read requirement
        path = Path(arg)
        if path.exists():
            requirement_yaml = path.read_text()
            brief = generate_brief(requirement_yaml, requirement_path=str(path))
        else:
            requirement_yaml = arg
            brief = generate_brief(requirement_yaml)
        
        # Output brief
        print(json.dumps(brief, indent=2, ensure_ascii=False))
        
    elif cmd == 'verify':
        path = Path(arg)
        brief = json.loads(path.read_text())
        verify_brief(brief)
        
    elif cmd == 'cmd':
        if len(sys.argv) < 4:
            print("Usage: brief.py cmd '<action>' '<args_json>'")
            sys.exit(1)
        action = sys.argv[2]
        args = json.loads(sys.argv[3])
        cmd = command_to_vcli(action, args)
        print(cmd)
    
    else:
        print(f"Unknown command: {cmd}")
        sys.exit(1)


if __name__ == '__main__':
    main()
