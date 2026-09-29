#!/usr/bin/python3
"""
verify.py — Verify Execution Brief against SHA-256 and specs

Usage:
    python3 verify.py <brief.json>
    python3 verify.py <brief.json> --requirement <requirement.yaml>
"""

import hashlib
import json
import os
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional


def sha256_content(content: str) -> str:
    """Compute SHA-256 hex digest."""
    return hashlib.sha256(content.encode()).hexdigest()


def check_results_exist(path: str) -> bool:
    """Check if results directory exists."""
    return os.path.exists(path)


def check_psf_files(pattern: str, directory: str) -> bool:
    """Check if PSF files matching pattern exist."""
    if not os.path.exists(directory):
        return False
    
    # Simple glob matching
    import fnmatch
    files = os.listdir(directory)
    matches = fnmatch.filter(files, pattern)
    return len(matches) > 0


def verify_brief(brief: Dict[str, Any], requirement_yaml: Optional[str] = None) -> Dict[str, Any]:
    """
    Verify execution brief.
    
    Args:
        brief: Brief to verify
        requirement_yaml: Optional requirement YAML for SHA-256 re-check
    
    Returns:
        Verification result dict
    """
    verification = {
        'passed': True,
        'sha256_valid': True,
        'checks': []
    }
    
    # 1. Check SHA-256
    if requirement_yaml:
        expected_sha256 = sha256_content(requirement_yaml)
        sha256_match = brief.get('sha256') == expected_sha256
        verification['sha256_valid'] = sha256_match
        
        if sha256_match:
            verification['checks'].append({
                'name': 'sha256',
                'passed': True,
                'detail': f'Matches: {expected_sha256[:16]}...'
            })
        else:
            verification['passed'] = False
            verification['checks'].append({
                'name': 'sha256',
                'passed': False,
                'detail': f'Expected {expected_sha256[:16]}..., got {brief.get("sha256", "")[:16]}...'
            })
    
    # 2. Check status
    status = brief.get('status')
    if status == 'success':
        verification['checks'].append({
            'name': 'execution_status',
            'passed': True,
            'detail': f'Status: {status}'
        })
    elif status == 'failed':
        verification['passed'] = False
        verification['checks'].append({
            'name': 'execution_status',
            'passed': False,
            'detail': f'Failed at command {brief.get("failed_at", "?")}: {brief.get("failure_reason", "Unknown")}'
        })
    else:
        verification['checks'].append({
            'name': 'execution_status',
            'passed': False,
            'detail': f'Unexpected status: {status}'
        })
    
    # 3. Check artifacts
    artifacts = brief.get('artifacts', {})
    outputs = brief.get('requirement', {}).get('outputs', [])
    
    for output in outputs:
        output_type = output.get('type')
        
        if output_type == 'maestro_session':
            session = artifacts.get('maestro_session')
            verification['checks'].append({
                'name': f'output_{output_type}',
                'passed': bool(session),
                'detail': f'Maestro session: {session or "not found"}'
            })
            if not session:
                verification['passed'] = False
        
        elif output_type == 'results_dir':
            results_dir = artifacts.get('results_dir')
            exists = results_dir and check_results_exist(results_dir)
            verification['checks'].append({
                'name': f'output_{output_type}',
                'passed': exists,
                'detail': f'Results directory: {results_dir or "not set"} ({("exists" if exists else "missing")})'
            })
            if not exists:
                verification['passed'] = False
        
        elif output_type == 'psf_files':
            pattern = output.get('pattern', '*.psf')
            results_dir = artifacts.get('results_dir')
            if results_dir:
                found = check_psf_files(pattern, results_dir)
                verification['checks'].append({
                    'name': f'output_{output_type}',
                    'passed': found,
                    'detail': f'PSF files ({pattern}): {("found" if found else "not found")} in {results_dir}'
                })
                if not found:
                    verification['passed'] = False
    
    # 4. Check specs (if results available)
    specs = brief.get('requirement', {}).get('specs', [])
    results_dir = artifacts.get('results_dir')
    
    if specs and results_dir and os.path.exists(results_dir):
        spec_results = verify_specs(specs, results_dir)
        verification['checks'].extend(spec_results)
        
        for check in spec_results:
            if not check['passed']:
                verification['passed'] = False
    
    return verification


def verify_specs(specs: List[Dict], results_dir: str) -> List[Dict]:
    """
    Verify specs against simulation results.
    
    Note: This is a stub. Actual implementation would:
    1. Parse PSF files using maestro-read-results skill
    2. Extract measured values
    3. Compare against spec min/max/target
    
    Args:
        specs: List of spec definitions
        results_dir: Directory containing PSF results
    
    Returns:
        List of verification check results
    """
    checks = []
    
    for spec in specs:
        spec_name = spec.get('name', 'unknown')
        
        # Check if measurement file exists
        # This would call maestro-read-results skill
        
        checks.append({
            'name': f'spec_{spec_name}',
            'passed': None,  # Unknown without actual measurement
            'detail': f'Spec check requires PSF parsing (not implemented in verify.py)'
        })
    
    return checks


def print_verification(verification: Dict[str, Any]) -> None:
    """Print verification results in human-readable format."""
    print("\n" + "=" * 60)
    print("VERIFICATION RESULTS")
    print("=" * 60)
    
    overall = "✓ PASSED" if verification['passed'] else "✗ FAILED"
    print(f"\nOverall: {overall}")
    print(f"SHA-256 Valid: {'✓' if verification['sha256_valid'] else '✗'}")
    
    print("\nChecks:")
    for check in verification.get('checks', []):
        icon = '✓' if check['passed'] else '✗' if check['passed'] is False else '?'
        print(f"  [{icon}] {check['name']}: {check.get('detail', '')}")
    
    print("\n" + "=" * 60)


def main():
    """CLI entry point."""
    if len(sys.argv) < 2:
        print("Usage:")
        print("  verify.py <brief.json>")
        print("  verify.py <brief.json> --requirement <requirement.yaml>")
        sys.exit(1)
    
    brief_path = sys.argv[1]
    requirement_path = None
    
    for i, arg in enumerate(sys.argv):
        if arg == '--requirement' and i + 1 < len(sys.argv):
            requirement_path = sys.argv[i + 1]
    
    # Load brief
    brief = json.loads(Path(brief_path).read_text())
    
    # Load requirement if provided
    requirement_yaml = None
    if requirement_path:
        requirement_yaml = Path(requirement_path).read_text()
    
    # Verify
    verification = verify_brief(brief, requirement_yaml)
    
    # Print results
    print_verification(verification)
    
    # Exit with appropriate code
    sys.exit(0 if verification['passed'] else 1)


if __name__ == '__main__':
    main()
