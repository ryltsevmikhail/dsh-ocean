#!/usr/bin/env python3
"""Send a real WM_DELETE_WINDOW request in the private Xvfb smoke only.

Unlike destroying an X window or killing its process, this exercises Tauri's
CloseRequested handler. No window-manager process or third-party Python module
is needed. This is never used by the shipped client.
"""
import argparse
import ctypes
import os
from pathlib import Path


class MessageData(ctypes.Union):
    _fields_ = [("bytes", ctypes.c_char * 20), ("shorts", ctypes.c_short * 10), ("longs", ctypes.c_long * 5)]


class ClientMessage(ctypes.Structure):
    _fields_ = [
        ("type", ctypes.c_int), ("serial", ctypes.c_ulong),
        ("send_event", ctypes.c_int), ("display", ctypes.c_void_p),
        ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
        ("format", ctypes.c_int), ("data", MessageData),
    ]


class Event(ctypes.Union):
    _fields_ = [("client", ClientMessage), ("padding", ctypes.c_long * 24)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("window", type=lambda value: int(value, 16))
    arguments = parser.parse_args()
    root = Path(os.environ.get("XDG_RUNTIME_DIR", ""))
    if (os.environ.get("DSH_OCEAN_TEST_X11") != "1" or not root.is_absolute()
            or root.name != "runtime" or root.parent.parent != Path("/tmp")
            or not root.parent.name.startswith("dsh-ocean-native-test.")
            or root.is_symlink() or root.parent.is_symlink() or not root.is_dir()
            or not os.environ.get("DISPLAY")):
        parser.error("requires an explicitly isolated native Xvfb fixture")
    if arguments.window <= 0:
        parser.error("expected a nonzero X11 window ID")
    x11 = ctypes.CDLL("libX11.so.6")
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.POINTER(Event)]
    x11.XSendEvent.restype = ctypes.c_int
    x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x11.XOpenDisplay(None)
    if not display:
        parser.error("cannot open the isolated X11 display")
    try:
        event = Event()
        event.client.type = 33  # ClientMessage
        event.client.send_event = 1
        event.client.display = display
        event.client.window = arguments.window
        event.client.message_type = x11.XInternAtom(display, b"WM_PROTOCOLS", 0)
        event.client.format = 32
        event.client.data.longs[0] = x11.XInternAtom(display, b"WM_DELETE_WINDOW", 0)
        event.client.data.longs[1] = 0  # CurrentTime
        if not x11.XSendEvent(display, arguments.window, 0, 0, ctypes.byref(event)):
            parser.error("could not send the window-close request")
        x11.XSync(display, 0)
    finally:
        x11.XCloseDisplay(display)


if __name__ == "__main__":
    main()
