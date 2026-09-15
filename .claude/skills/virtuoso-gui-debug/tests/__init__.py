import os
import sys

# Ensure the skill root (parent of tests/) is importable so that
# `from vgui_runner.model import ...` resolves. The vgui_runner package was
# moved from scripts/ to the skill root to satisfy the 2-level Skill packaging
# directory limit (root / second-level dir / file); unittest's discover only
# injects scripts/ and tests/ into sys.path, not the skill root.
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
