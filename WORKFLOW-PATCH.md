# Skill test workflow

The workflow includes path filters for both skill trees, the sync tool, and
its regression tests. It uses Ubuntu 24.04 to run Python 3.9 and 3.13, and
installs the image-processing dependencies used by virastack-auto-allow tests.
The drift gate uses `--strict-pending`, so new skill directories require an
explicit disposition before CI passes. CI results remain authoritative.
