"""Minimal plugin API contract check used by `pnpm env:doctor`.

Adapted from upstream `verify-pymss-plugin-api.py`. Unlike the release-time check,
this one never fails a fresh checkout: an interpreter without the bundled plugin
packages simply reports `skip`, because those packages arrive with the runtime.
"""
from __future__ import annotations

from importlib import import_module, metadata
import json

REQUIRED = {
    "pymss.plugins.kit": (
        "current_operation_context", "run_native_process",
        "register_commands", "register_rpc_processes",
    ),
    "pymss_plugin_manager": ("PluginContext", "PluginPolicy", "PluginService"),
    "pymss.plugins.marketplace": ("fetch_official_catalog", "source_info"),
}


def verify() -> dict:
    try:
        pymss_version = metadata.version("pymss")
    except metadata.PackageNotFoundError:
        return {"status": "skip", "detail": "pymss is not installed in this interpreter yet"}
    missing: list[str] = []
    for name, attributes in REQUIRED.items():
        try:
            module = import_module(name)
        except Exception as exc:
            missing.append(f"{name} ({type(exc).__name__})")
            continue
        for attribute in attributes:
            if not callable(getattr(module, attribute, None)):
                missing.append(f"{name}.{attribute}")
    try:
        reviewed_update = getattr(import_module("pymss_plugin_manager._engine"), "UPDATE_REVIEW_VERSION", None)
        if type(reviewed_update) is not int or reviewed_update != 1:
            missing.append("pymss_plugin_manager._engine.UPDATE_REVIEW_VERSION=1")
    except Exception as exc:
        missing.append(f"pymss_plugin_manager._engine ({type(exc).__name__})")
    if missing:
        return {"status": "fail", "detail": "missing plugin APIs: " + ", ".join(missing)}
    return {"status": "ok", "detail": f"pymss {pymss_version}: plugin APIs available"}


if __name__ == "__main__":
    print(json.dumps(verify(), ensure_ascii=False))
