#!/usr/bin/env python3
"""Replace a staged app without deleting the bundle backing a running process."""
import fcntl
from pathlib import Path
import shutil
import subprocess
import sys
import uuid


def prune_retired(directory):
    if not directory.exists():
        return
    for bundle in directory.glob("*.app"):
        result = subprocess.run(
            ["/usr/sbin/lsof", "-t", "+D", str(bundle)],
            capture_output=True, text=True,
        )
        # lsof returns 1 with no output when no process holds this directory.
        # On any inspection failure, retain it rather than risk a live identity.
        if result.returncode == 1 and not result.stdout and not result.stderr:
            shutil.rmtree(bundle)


def install(source, destination):
    source = source.resolve(strict=True)
    destination = destination.absolute()
    destination.parent.mkdir(parents=True, exist_ok=True)
    retired = destination.parent / ".del-retired"
    retired.mkdir(exist_ok=True)
    with (destination.parent / ".del-install.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        prune_retired(retired)
        previous = None
        if destination.exists():
            previous = retired / f"del-{uuid.uuid4()}.app"
            destination.rename(previous)
        try:
            source.rename(destination)
        except OSError:
            if previous is not None:
                previous.rename(destination)
            raise
        prune_retired(retired)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("Usage: install-macos.py staged.app destination.app")
    install(Path(sys.argv[1]), Path(sys.argv[2]))
