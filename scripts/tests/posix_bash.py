"""The POSIX bash a script guard runs a shell script with.

On macOS and Linux it is the bash PATH finds. On Windows a bare "bash" is no good: CreateProcess
searches the system directory before PATH, and System32's bash.exe is WSL's, which cannot open a
Windows path; PATH order alone is no better, because a stock Windows PATH lists System32 before
Git. So on Windows: HIWAVE_BASH when set, else a bash on PATH that is not under the Windows
directory, else Git for Windows' own bash; None when there is none (the caller skips, saying so).
"""
import os
import shutil
from pathlib import Path


def posix_bash():
    if os.name != "nt":
        return shutil.which("bash") or "bash"
    explicit = os.environ.get("HIWAVE_BASH")
    if explicit and Path(explicit).is_file():
        return explicit
    windows_dir = Path(os.environ.get("SystemRoot", r"C:\Windows")).resolve()
    for entry in os.environ.get("PATH", "").split(os.pathsep):
        candidate = Path(entry) / "bash.exe"
        if candidate.is_file() and not candidate.resolve().is_relative_to(windows_dir) \
                and "WindowsApps" not in candidate.parts:
            return str(candidate)
    for root in (os.environ.get("ProgramFiles"), os.environ.get("ProgramW6432")):
        if root:
            for rel in (r"Git\bin\bash.exe", r"Git\usr\bin\bash.exe"):
                candidate = Path(root) / rel
                if candidate.is_file():
                    return str(candidate)
    return None
