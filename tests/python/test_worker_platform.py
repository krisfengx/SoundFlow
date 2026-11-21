from __future__ import annotations

import os
import types
import unittest
from unittest import mock

if __package__:
    from . import _bootstrap as _worker_test_bootstrap  # noqa: F401
else:
    import _bootstrap as _worker_test_bootstrap  # noqa: F401

import worker_platform


class PlatformFactTests(unittest.TestCase):
    def test_windows_machine_prefers_the_native_architecture(self):
        with mock.patch.object(worker_platform.sys, 'platform', 'win32'), \
             mock.patch.dict(os.environ, {
                 'PROCESSOR_ARCHITEW6432': 'AMD64',
                 'PROCESSOR_ARCHITECTURE': 'x86',
             }):
            self.assertEqual(worker_platform.machine(), 'AMD64')

    def test_windows_machine_falls_back_to_the_emulated_architecture(self):
        with mock.patch.object(worker_platform.sys, 'platform', 'win32'), \
             mock.patch.dict(os.environ, {'PROCESSOR_ARCHITECTURE': 'ARM64'}, clear=True):
            self.assertEqual(worker_platform.machine(), 'ARM64')

    def test_windows_description_includes_version_and_architecture(self):
        version = types.SimpleNamespace(major=10, minor=0, build=19045)
        with mock.patch.object(worker_platform.sys, 'platform', 'win32'), \
             mock.patch.object(worker_platform.sys, 'getwindowsversion', create=True, return_value=version), \
             mock.patch.dict(os.environ, {'PROCESSOR_ARCHITECTURE': 'AMD64'}, clear=True):
            self.assertEqual(worker_platform.description(), 'Windows-10.0.19045-AMD64')

    def test_non_windows_uses_the_standard_library(self):
        with mock.patch.object(worker_platform.sys, 'platform', 'linux'), \
             mock.patch.object(worker_platform.platform, 'machine', return_value='x86_64'):
            self.assertEqual(worker_platform.machine(), 'x86_64')
