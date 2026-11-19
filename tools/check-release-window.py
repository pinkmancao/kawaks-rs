"""Verify that the release executable creates a visible native SDL2 window."""

import os
import re
import subprocess
import sys
import time
from pathlib import Path


def windows_visible(pid):
    import ctypes
    from ctypes import wintypes

    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user32 = ctypes.WinDLL("user32")
    user32.EnumWindows.argtypes = (callback_type, wintypes.LPARAM)
    user32.IsWindowVisible.argtypes = (wintypes.HWND,)
    user32.GetWindowThreadProcessId.argtypes = (wintypes.HWND, ctypes.POINTER(wintypes.DWORD))
    user32.GetWindowRect.argtypes = (wintypes.HWND, ctypes.POINTER(wintypes.RECT))
    visible = False

    @callback_type
    def check(hwnd, _):
        nonlocal visible
        owner = wintypes.DWORD()
        size = wintypes.RECT()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if (
            owner.value == pid
            and user32.IsWindowVisible(hwnd)
            and user32.GetWindowRect(hwnd, ctypes.byref(size))
            and size.right > size.left
            and size.bottom > size.top
        ):
            visible = True
            return False
        return True

    user32.EnumWindows(check, 0)
    return visible


def x11_visible(pid):
    window = subprocess.run(
        ["xwininfo", "-name", "Kawaks Rust"], capture_output=True, text=True, timeout=3
    ).stdout
    match = re.search(r"Window id: (0x[0-9a-f]+)", window)
    if not match or "Map State: IsViewable" not in window:
        return False
    owner = subprocess.run(
        ["xprop", "-id", match[1], "_NET_WM_PID"], capture_output=True, text=True, timeout=3
    ).stdout
    return re.search(rf"=\s*{pid}\s*$", owner) is not None


def main():
    binary = Path(sys.argv[1]).resolve(strict=True)
    backend, visible = (
        ("windows", windows_visible) if sys.platform == "win32" else ("x11", x11_visible)
    )
    with subprocess.Popen([binary, "--gui"], env={**os.environ, "SDL_VIDEODRIVER": backend}) as app:
        try:
            deadline = time.monotonic() + 30
            consecutive = 0
            while time.monotonic() < deadline and app.poll() is None:
                consecutive = consecutive + 1 if visible(app.pid) else 0
                if consecutive == 2:
                    print(f"Verified visible {backend} launcher window for PID {app.pid}.")
                    return 0
                time.sleep(0.2)
            print(f"No visible {backend} launcher window (process status {app.poll()}).", file=sys.stderr)
            return 1
        finally:
            if app.poll() is None:
                app.terminate()
                try:
                    app.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    app.kill()


if __name__ == "__main__":
    sys.exit(main())
