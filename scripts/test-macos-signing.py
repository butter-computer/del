#!/usr/bin/env python3
"""Prove a changed executable retains identity, while another key cannot impersonate it."""
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile

SIGN = Path(__file__).with_name("sign-macos.py")


def run(*args, **kwargs):
    result = subprocess.run(args, text=True, capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError(result.stderr)
    return result


def requirement(bundle):
    result = run("/usr/bin/codesign", "-d", "-r-", str(bundle))
    return (result.stdout + result.stderr).split("designated => ")[1].splitlines()[0].strip()


def main():
    if sys.platform != "darwin":
        print("macOS signing test requires macOS")
        return
    env = {k: v for k, v in os.environ.items() if not k.startswith("DEL_SIGNING_")}
    with tempfile.TemporaryDirectory(prefix="del-signing-test-") as temporary:
        root = Path(temporary)
        states = [root / "identity", root / "different-identity"]
        bundle = root / "Del.app"
        executable = bundle / "Contents/MacOS/del"
        executable.parent.mkdir(parents=True)
        with (bundle / "Contents/Info.plist").open("wb") as output:
            plistlib.dump({"CFBundleIdentifier": "tech.unordinary.del.signing-test",
                          "CFBundleExecutable": "del", "CFBundlePackageType": "APPL"}, output)
        source = root / "main.c"
        try:
            identities = []
            for version in (1, 2):
                source.write_text(f"int main(void) {{ return {version}; }}\n")
                run("/usr/bin/clang", str(source), "-o", str(executable))
                run(sys.executable, str(SIGN), str(bundle), "--identity-directory", str(states[0]), env=env)
                identities.append(requirement(bundle))
            assert identities[0] == identities[1], "rebuild changed permission identity"
            assert "certificate leaf" in identities[0] and "cdhash" not in identities[0]
            run("/usr/bin/codesign", "--verify", "--strict", "-R", "=" + identities[0], str(bundle))
            run(sys.executable, str(SIGN), str(bundle), "--identity-directory", str(states[1]), env=env)
            mismatch = subprocess.run(["/usr/bin/codesign", "--verify", "-R", "=" + identities[0], str(bundle)], capture_output=True)
            assert mismatch.returncode != 0, "another signing key must not inherit permission identity"
            states[0].joinpath("certificate.pem").unlink()
            incomplete = subprocess.run([sys.executable, str(SIGN), str(bundle), "--identity-directory", str(states[0])], env=env, capture_output=True)
            assert incomplete.returncode != 0, "incomplete identity must fail, not silently regenerate"
            print("PASS: changed builds retain identity; other keys rejected; incomplete identity fails closed")
        finally:
            for state in states:
                keychain = state / "identity.keychain-db"
                if keychain.exists():
                    run("/usr/bin/security", "delete-keychain", str(keychain))


if __name__ == "__main__":
    main()
