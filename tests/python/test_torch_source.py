"""PyTorch wheel source selection for runtime installation."""
from __future__ import annotations

import unittest

from . import _bootstrap  # noqa: F401  (puts python/ on sys.path)

import worker_bootstrap as wb


class TorchSourceTests(unittest.TestCase):
    def test_variant_comes_only_from_official_pytorch_index(self):
        self.assertEqual(wb._torch_index_variant("https://download.pytorch.org/whl/cpu"), "cpu")
        self.assertEqual(wb._torch_index_variant("https://download.pytorch.org/whl/cu128"), "cu128")
        self.assertIsNone(wb._torch_index_variant("https://download.pytorch.org/whl/rocm7.1"))
        self.assertIsNone(wb._torch_index_variant("https://stable.repo.amd.com/rocm/whl-next/"))
        self.assertIsNone(wb._torch_index_variant(None))

    def test_sjtu_replaces_index_url(self):
        index, links = wb._resolve_torch_index("https://download.pytorch.org/whl/cu128", "sjtu")
        self.assertEqual(index, "https://mirror.sjtu.edu.cn/pytorch-wheels/cu128/")
        self.assertIsNone(links)

    def test_aliyun_uses_find_links(self):
        index, links = wb._resolve_torch_index("https://download.pytorch.org/whl/cpu", "aliyun")
        self.assertIsNone(index)
        self.assertEqual(links, "https://mirrors.aliyun.com/pytorch-wheels/cpu/")

    def test_official_and_amd_sources_are_never_rewritten(self):
        official = "https://download.pytorch.org/whl/cpu"
        self.assertEqual(wb._resolve_torch_index(official, "official"), (official, None))
        unknown = "https://download.pytorch.org/whl/cu128"
        self.assertEqual(wb._resolve_torch_index(unknown, "unknown-source"), (unknown, None))
        amd = "https://repo.radeon.com/rocm/windows/rocm-rel-7.2.1/torch.whl"
        self.assertEqual(wb._resolve_torch_index(amd, "sjtu"), (amd, None))


if __name__ == "__main__":
    unittest.main()
