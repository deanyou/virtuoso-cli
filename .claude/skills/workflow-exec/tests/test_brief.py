#!/usr/bin/env python3
"""Tests for workflow-exec scripts."""

import json
import sys
import unittest
from pathlib import Path

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent.parent / 'scripts'))

from brief import generate_brief, sha256_content, command_to_vcli, verify_brief


class TestBriefGeneration(unittest.TestCase):
    """Test brief generation."""
    
    def test_sha256_content(self):
        """Test SHA-256 computation."""
        content = "test content"
        hash1 = sha256_content(content)
        hash2 = sha256_content(content)
        
        self.assertEqual(hash1, hash2)
        self.assertEqual(len(hash1), 64)  # SHA-256 hex is 64 chars
    
    def test_different_content_different_hash(self):
        """Test that different content produces different hashes."""
        hash1 = sha256_content("content1")
        hash2 = sha256_content("content2")
        self.assertNotEqual(hash1, hash2)
    
    def test_generate_brief(self):
        """Test brief generation from YAML."""
        yaml_content = """
id: test_workflow
testbench:
  lib: TEST_LIB
  cell: TEST_CELL
commands:
  - action: maestro.open_session
  - action: maestro.run
"""
        brief = generate_brief(yaml_content)
        
        self.assertEqual(brief['id'], 'test_workflow')
        self.assertEqual(brief['status'], 'pending')
        self.assertIn('sha256', brief)
        self.assertIn('created', brief)
        self.assertEqual(len(brief['commands']), 0)  # Commands not executed yet
    
    def test_auto_id_generation(self):
        """Test automatic ID generation when not specified."""
        yaml_content = """
testbench:
  lib: TEST_LIB
"""
        brief = generate_brief(yaml_content)
        
        self.assertTrue(brief['id'].startswith('brief-'))
        self.assertIn('2026', brief['created'])  # Current year


class TestCommandConversion(unittest.TestCase):
    """Test vcli command conversion."""
    
    def test_maestro_open_session(self):
        """Test maestro.open_session conversion."""
        cmd = command_to_vcli('maestro.open_session', {
            'lib': 'TEST_LIB',
            'cell': 'TEST_CELL'
        })
        self.assertIn('vcli', cmd)
        self.assertIn('maestro open-session', cmd)
    
    def test_maestro_run(self):
        """Test maestro.run conversion."""
        cmd = command_to_vcli('maestro.run', {})
        self.assertEqual(cmd, 'vcli maestro run')
    
    def test_cell_open(self):
        """Test cell.open conversion."""
        cmd = command_to_vcli('cell.open', {
            'lib': 'mylib',
            'cell': 'mycell',
            'view': 'schematic'
        })
        self.assertIn('vcli cell open', cmd)


class TestBriefVerification(unittest.TestCase):
    """Test brief verification."""
    
    def test_verify_valid_brief(self):
        """Test verification of valid brief."""
        brief = {
            'id': 'test-brief',
            'sha256': sha256_content('test content'),
            'created': '2026-09-28T00:00:00Z',
            'requirement': {},
            'status': 'success',
            'commands': [],
            'artifacts': {},
            'verification': {}
        }
        
        # Without requirement YAML, returns bool
        result = verify_brief(brief, requirement_yaml=None)
        self.assertTrue(result)
    
    def test_verify_sha256_mismatch(self):
        """Test verification catches SHA-256 mismatch."""
        brief = {
            'id': 'test-brief',
            'sha256': 'wrong_hash',
            'created': '2026-09-28T00:00:00Z',
            'requirement': {},
            'status': 'success',
            'commands': [],
            'artifacts': {},
            'verification': {}
        }
        
        result = verify_brief(brief, requirement_yaml='test content')
        self.assertFalse(result)
    
    def test_verify_failed_status(self):
        """Test verification catches failed status."""
        brief = {
            'id': 'test-brief',
            'sha256': 'any_hash',
            'created': '2026-09-28T00:00:00Z',
            'requirement': {},
            'status': 'failed',
            'failed_at': 1,
            'failure_reason': 'Command timed out',
            'commands': [{'success': False}],
            'artifacts': {},
            'verification': {}
        }
        
        result = verify_brief(brief)
        self.assertFalse(result)


if __name__ == '__main__':
    unittest.main()
