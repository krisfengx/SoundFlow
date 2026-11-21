from __future__ import annotations

import unittest
from unittest import mock

if __package__:
    from . import _bootstrap as _worker_test_bootstrap  # noqa: F401
else:
    import _bootstrap as _worker_test_bootstrap  # noqa: F401

import worker_network_diagnostics as net


class _Response:
    def __init__(self, status: int = 200) -> None:
        self.status = status

    def read(self, limit: int) -> bytes:
        return b'ok'

    def __enter__(self):
        return self

    def __exit__(self, *args) -> bool:
        return False


class NetworkDiagnosticsTests(unittest.TestCase):
    def test_a_successful_probe_reports_host_and_elapsed_time(self):
        with mock.patch.object(net, 'proxy_urlopen', return_value=_Response(200)):
            result = net._check('models', 'modelscope', 'https://www.modelscope.cn/api/x', None)
        self.assertEqual(result['status'], 'ok')
        self.assertEqual(result['host'], 'www.modelscope.cn')
        self.assertIn('elapsedMs', result)

    def test_a_non_2xx_response_is_reported_as_failed(self):
        with mock.patch.object(net, 'proxy_urlopen', return_value=_Response(503)):
            result = net._check('pypi', 'pypi', 'https://pypi.org/simple/pymss/', None)
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['httpStatus'], 503)

    def test_an_unknown_source_is_rejected_before_any_request(self):
        with mock.patch.object(net, 'emit_error', return_value=1) as emit_error, \
             mock.patch.object(net, 'proxy_urlopen') as request:
            code = net.cmd_network_diagnostics({'modelSource': 'unknown', 'locale': 'zh-CN'})
        self.assertEqual(code, 1)
        self.assertEqual(emit_error.call_args[0][0], 'NETWORK_DIAGNOSTICS_FAILED')
        request.assert_not_called()

    def test_the_command_probes_both_the_model_source_and_the_pypi_mirror(self):
        config = mock.Mock(mode='system')
        with mock.patch.object(net, 'load_proxy_config', return_value=config), \
             mock.patch.object(net, 'configure_process_proxy'), \
             mock.patch.object(net, 'proxy_urlopen', return_value=_Response(200)), \
             mock.patch.object(net, 'emit') as emit:
            code = net.cmd_network_diagnostics({'modelSource': 'modelscope', 'mirror': 'auto', 'locale': 'zh-CN'})
        self.assertEqual(code, 0)
        payload = emit.call_args[0][1]
        self.assertEqual([item['id'] for item in payload['checks']], ['models', 'pypi'])
        self.assertEqual(payload['proxyMode'], 'system')
        self.assertEqual(payload['timeoutSeconds'], net.TIMEOUT)
