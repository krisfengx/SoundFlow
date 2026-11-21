"""Platform facts that do not depend on Windows Management Instrumentation.

Adapted from upstream pymss-studio. ``platform.machine()`` on Windows can fall back
to a WMI query, which is slow and occasionally unavailable in restricted sessions;
the architecture environment variables are already populated by the OS loader.
"""
from __future__ import annotations

import os
import platform
import sys


def machine() -> str:
    if sys.platform == "win32":
        # Match platform.machine's native-architecture fallback, including WOW64.
        return os.environ.get("PROCESSOR_ARCHITEW6432") or os.environ.get("PROCESSOR_ARCHITECTURE", "")
    return platform.machine()


def description() -> str:
    if sys.platform == "win32":
        version = sys.getwindowsversion()
        return f"Windows-{version.major}.{version.minor}.{version.build}-{machine()}"
    return platform.platform()
