#!/usr/bin/env python3
"""Sign Del's public update manifest (latest.json) with the release Ed25519 key.

The updater accepts a release only when `signature` verifies, with the public
key embedded in the app, over the exact tag, DMG name, size, and SHA-256.

  sign-update.py generate                         create the key once (never overwrites)
  sign-update.py public-key                       print the public key for app_updates.rs
  sign-update.py manifest VERSION DMG OUTPUT      write a signed latest.json

The private key is read from $DEL_UPDATE_SIGNING_KEY, default
~/.config/del/update-signing.key (64 hex characters, mode 0600). Keep it out
of the repository and back it up: losing it means existing installs can no
longer accept updates.

Ed25519 is implemented here (RFC 8032 reference algorithm) so release builds
need only the system Python; it is checked against the RFC test vector and
every signature is verified before it is written.
"""
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
UPDATER = REPO / "crates/app/src/app_updates.rs"
RELEASES = "https://github.com/butter-computer/del/releases/download"

# ---- Ed25519 (RFC 8032, section 6) ----
P = 2**255 - 19
Q = 2**252 + 27742317777372353535851937790883648493
D = -121665 * pow(121666, -1, P) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)


def _add(a, b):
    x = (a[1] - a[0]) * (b[1] - b[0]) % P
    y = (a[1] + a[0]) * (b[1] + b[0]) % P
    c = 2 * a[3] * b[3] * D % P
    d = 2 * a[2] * b[2] % P
    e, f, g, h = y - x, d - c, d + c, y + x
    return (e * f, g * h, f * g, e * h)


def _mul(s, point):
    result = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            result = _add(result, point)
        point = _add(point, point)
        s >>= 1
    return result


def _equal(a, b):
    return (a[0] * b[2] - b[0] * a[2]) % P == 0 and (a[1] * b[2] - b[1] * a[2]) % P == 0


def _recover_x(y, sign):
    if y >= P:
        return None
    x2 = (y * y - 1) * pow(D * y * y + 1, -1, P)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P:
        x = x * SQRT_M1 % P
    if (x * x - x2) % P:
        return None
    if (x & 1) != sign:
        x = P - x
    return x


_GY = 4 * pow(5, -1, P) % P
_GX = _recover_x(_GY, 0)
G = (_GX, _GY, 1, _GX * _GY % P)


def _compress(point):
    zinv = pow(point[2], -1, P)
    x, y = point[0] * zinv % P, point[1] * zinv % P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def _decompress(data):
    if len(data) != 32:
        return None
    y = int.from_bytes(data, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    return None if x is None else (x, y, 1, x * y % P)


def _expand(secret):
    digest = hashlib.sha512(secret).digest()
    a = int.from_bytes(digest[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, digest[32:]


def public_key(secret):
    return _compress(_mul(_expand(secret)[0], G))


def sign(secret, message):
    a, prefix = _expand(secret)
    public = _compress(_mul(a, G))
    r = int.from_bytes(hashlib.sha512(prefix + message).digest(), "little") % Q
    encoded_r = _compress(_mul(r, G))
    h = int.from_bytes(hashlib.sha512(encoded_r + public + message).digest(), "little") % Q
    return encoded_r + int.to_bytes((r + h * a) % Q, 32, "little")


def verify(public, message, signature):
    point = _decompress(public)
    r = _decompress(signature[:32])
    if point is None or r is None or len(signature) != 64:
        return False
    s = int.from_bytes(signature[32:], "little")
    if s >= Q:
        return False
    h = int.from_bytes(hashlib.sha512(signature[:32] + public + message).digest(), "little") % Q
    return _equal(_mul(s, G), _add(r, _mul(h, point)))


def _self_test():
    # RFC 8032 section 7.1, test 1.
    secret = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
    expected = bytes.fromhex(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155"
        "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
    public = public_key(secret)
    if public.hex() != "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a" \
            or sign(secret, b"") != expected or not verify(public, b"", expected) \
            or verify(public, b"x", expected):
        raise SystemExit("Ed25519 self-test failed; refusing to sign.")


# ---- Del manifest ----
def signed_message(tag, name, size, sha256):
    """Must match `signed_message` in crates/app/src/app_updates.rs exactly."""
    return f"del-update-v1\n{tag}\n{name}\n{size}\n{sha256}\n".encode()


def key_path():
    return Path(os.environ.get("DEL_UPDATE_SIGNING_KEY")
                or Path.home() / ".config/del/update-signing.key")


def read_key():
    path = key_path()
    if not path.is_file():
        raise SystemExit(f"Update signing key not found at {path}. Restore it from your backup "
                         "(or set DEL_UPDATE_SIGNING_KEY); only create a new key with "
                         "`scripts/sign-update.py generate` if you also ship a build embedding it.")
    if stat.S_IMODE(path.stat().st_mode) & 0o077:
        raise SystemExit(f"{path} must not be readable by other users (chmod 600).")
    text = path.read_text().strip()
    if not re.fullmatch(r"[0-9a-f]{64}", text):
        raise SystemExit(f"{path} is not a 64-character hex Ed25519 seed.")
    return bytes.fromhex(text)


def embedded_public_key():
    match = re.search(r'UPDATE_PUBLIC_KEY: &str =\s*"([0-9a-f]{64})"', UPDATER.read_text())
    if not match:
        raise SystemExit(f"Could not find UPDATE_PUBLIC_KEY in {UPDATER}")
    return match.group(1)


def generate():
    path = key_path()
    if path.exists():
        print(f"Keeping existing key at {path}", file=sys.stderr)
    else:
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(path.parent, 0o700)
        seed = os.urandom(32)
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as output:
            output.write(seed.hex() + "\n")
        print(f"Created {path}. Back it up somewhere safe and private.", file=sys.stderr)
    print(public_key(read_key()).hex())


def manifest(version, dmg, output):
    dmg = Path(dmg)
    secret = read_key()
    public = public_key(secret)
    if public.hex() != embedded_public_key():
        raise SystemExit(f"{key_path()} does not match UPDATE_PUBLIC_KEY in {UPDATER}; "
                         "installed apps would reject this release.")
    tag = f"v{version}"
    digest = hashlib.sha256(dmg.read_bytes()).hexdigest()
    size = dmg.stat().st_size
    signature = sign(secret, signed_message(tag, dmg.name, size, digest))
    if not verify(public, signed_message(tag, dmg.name, size, digest), signature):
        raise SystemExit("Signature verification failed; refusing to write the manifest.")
    assets = []
    for path in (dmg, Path(str(dmg) + ".sha256")):
        assets.append(dict(name=path.name, size=path.stat().st_size,
                           browser_download_url=f"{RELEASES}/{tag}/{path.name}"))
    data = dict(tag_name=tag, draft=False, prerelease=False, assets=assets,
                sha256=digest, signature=signature.hex())
    Path(output).write_text(json.dumps(data, indent=2) + "\n")


def main():
    _self_test()
    args = sys.argv[1:]
    if args == ["generate"]:
        generate()
    elif args == ["public-key"]:
        print(public_key(read_key()).hex())
    elif len(args) == 4 and args[0] == "manifest":
        manifest(*args[1:])
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
