#!/usr/bin/env python3
"""Sign local Del builds with a persistent, private certificate-backed identity.

No TCC databases, system trust settings, or permission grants are modified.
A Developer ID / Apple Development identity may be supplied instead.
"""
import argparse
import fcntl
import os
from pathlib import Path
import plistlib
import re
import secrets
import subprocess
import sys
import tempfile


def run(args, *, secret=None, input_text=None):
    result = subprocess.run(args, input=input_text, text=True, capture_output=True)
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        if secret:
            detail = detail.replace(secret, "<redacted>")
        raise RuntimeError(f"{Path(args[0]).name} failed: {detail}")
    return result.stdout + result.stderr


def local_identity(directory):
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(directory, 0o700)
    keychain = directory / "identity.keychain-db"
    certificate = directory / "certificate.pem"
    password_file = directory / "keychain-password"
    files = (keychain, certificate, password_file)
    with (directory / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if not any(path.exists() for path in files):
            password = secrets.token_urlsafe(48)
            with password_file.open("x") as output:
                output.write(password)
            with tempfile.TemporaryDirectory(prefix="create-", dir=directory) as temporary:
                temporary = Path(temporary)
                key = temporary / "key.pem"
                identity = temporary / "identity.p12"
                run(["/usr/bin/openssl", "req", "-new", "-newkey", "rsa:3072",
                     "-nodes", "-x509", "-days", "3650",
                     "-subj", "/CN=Del Local Development/",
                     "-keyout", str(key), "-out", str(certificate),
                     "-addext", "keyUsage=critical,digitalSignature",
                     "-addext", "extendedKeyUsage=codeSigning",
                     "-addext", "basicConstraints=critical,CA:FALSE"])
                run(["/usr/bin/openssl", "pkcs12", "-export", "-inkey", str(key),
                     "-in", str(certificate), "-out", str(identity), "-passout", "stdin"],
                    input_text=password + "\n", secret=password)
                run(["/usr/bin/security", "create-keychain", "-p", password, str(keychain)], secret=password)
                run(["/usr/bin/security", "set-keychain-settings", "-lut", "21600", str(keychain)])
                run(["/usr/bin/security", "import", str(identity), "-k", str(keychain),
                     "-P", password, "-T", "/usr/bin/codesign"], secret=password)
                run(["/usr/bin/security", "set-key-partition-list", "-S", "apple-tool:,apple:",
                     "-s", "-k", password, str(keychain)], secret=password)
        elif not all(path.is_file() for path in files):
            raise RuntimeError(f"Incomplete signing identity in {directory}. Restore its files; "
                               "refusing to replace the key and invalidate existing permissions.")
        password = password_file.read_text()
        run(["/usr/bin/security", "unlock-keychain", "-p", password, str(keychain)], secret=password)
        fingerprint = run(["/usr/bin/openssl", "x509", "-in", str(certificate),
                           "-noout", "-fingerprint", "-sha1"]).strip().split("=")[-1].replace(":", "")
        if not re.fullmatch(r"[A-Fa-f0-9]{40}", fingerprint):
            raise RuntimeError("Invalid signing certificate fingerprint")
        return fingerprint, keychain


def sign(bundle, identity_directory):
    bundle = bundle.resolve(strict=True)
    with (bundle / "Contents/Info.plist").open("rb") as source:
        identifier = plistlib.load(source)["CFBundleIdentifier"]
    identity = os.environ.get("DEL_SIGNING_IDENTITY")
    if identity == "-":
        raise RuntimeError("Ad-hoc signing cannot preserve permissions across builds")
    if identity:
        keychain = os.environ.get("DEL_SIGNING_KEYCHAIN")
    else:
        identity, keychain = local_identity(identity_directory)
    command = ["/usr/bin/codesign", "--force", "--sign", identity,
               "--identifier", identifier, "--timestamp=none"]
    if keychain:
        command += ["--keychain", str(keychain)]
    run(command + [str(bundle)])
    run(["/usr/bin/codesign", "--verify", "--strict", str(bundle)])
    requirement = run(["/usr/bin/codesign", "-d", "-r-", str(bundle)])
    if "certificate" not in requirement or "cdhash H" in requirement:
        raise RuntimeError("Signing did not produce a persistent certificate-backed identity")
    print(f"Signed {bundle.name} with persistent identity {identifier}")


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--identity-directory", type=Path,
                        default=Path.home() / "Library/Application Support/del/development-signing")
    args = parser.parse_args()
    try:
        sign(args.bundle, args.identity_directory.resolve())
    except (RuntimeError, OSError, KeyError) as error:
        parser.exit(1, f"Del signing failed: {error}\n")


if __name__ == "__main__":
    main()
