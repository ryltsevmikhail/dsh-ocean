"""Headless checks for the Xvfb-only WM_DELETE_WINDOW fixture helper."""
import contextlib
import ctypes
import io
import os
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch

HELPER = runpy.run_path(str(Path(__file__).resolve().parents[1] / "scripts/test-x11-close.py"))


class Function:
    def __init__(self, callback):
        self.callback = callback

    def __call__(self, *arguments):
        return self.callback(*arguments)


class X11CloseFixture(unittest.TestCase):
    def test_refuses_non_private_environment_before_loading_x11(self):
        for environment in ({}, {"DISPLAY": ":99", "DSH_OCEAN_TEST_X11": "1", "XDG_RUNTIME_DIR": "/tmp/not-a-native-fixture"}):
            with patch.dict(os.environ, environment, clear=True), patch.object(sys, "argv", ["close", "0x123"]), patch.object(ctypes, "CDLL") as library, contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as failure:
                    HELPER["main"]()
                self.assertEqual(failure.exception.code, 2)
                library.assert_not_called()

    def test_refuses_a_symlinked_runtime_before_loading_x11(self):
        with tempfile.TemporaryDirectory(prefix="dsh-ocean-native-test.", dir="/tmp") as temporary:
            runtime = Path(temporary) / "runtime"
            runtime.symlink_to("/tmp", target_is_directory=True)
            environment = {"DISPLAY": ":99", "DSH_OCEAN_TEST_X11": "1", "XDG_RUNTIME_DIR": str(runtime)}
            with patch.dict(os.environ, environment, clear=True), patch.object(sys, "argv", ["close", "0x123"]), patch.object(ctypes, "CDLL") as library, contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as failure:
                    HELPER["main"]()
                self.assertEqual(failure.exception.code, 2)
                library.assert_not_called()

    def test_sends_protocol_close_instead_of_destroying_or_killing(self):
        events = []

        def send(display, window, propagate, mask, pointer):
            event = ctypes.cast(pointer, ctypes.POINTER(HELPER["Event"])).contents.client
            events.append((display, window, propagate, mask, event.type, event.message_type, event.format, event.data.longs[0], event.data.longs[1]))
            return 1

        class Library:
            pass

        library = Library()
        library.XOpenDisplay = Function(lambda name: 7)
        library.XInternAtom = Function(lambda display, name, only: 10 if name == b"WM_PROTOCOLS" else 11)
        library.XSendEvent = Function(send)
        library.XSync = Function(lambda display, discard: events.append("sync"))
        library.XCloseDisplay = Function(lambda display: events.append("closed"))
        with tempfile.TemporaryDirectory(prefix="dsh-ocean-native-test.", dir="/tmp") as temporary:
            runtime = Path(temporary) / "runtime"
            runtime.mkdir()
            environment = {"DISPLAY": ":99", "DSH_OCEAN_TEST_X11": "1", "XDG_RUNTIME_DIR": str(runtime)}
            with patch.dict(os.environ, environment, clear=True), patch.object(sys, "argv", ["close", "0x123"]), patch.object(ctypes, "CDLL", return_value=library):
                HELPER["main"]()
        self.assertEqual(events, [(7, 0x123, 0, 0, 33, 10, 32, 11, 0), "sync", "closed"])


if __name__ == "__main__":
    unittest.main()
