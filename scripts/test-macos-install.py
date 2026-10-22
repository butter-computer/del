#!/usr/bin/env python3
"""Exercise macOS dynamic signature lookup while replacing a running app."""
from pathlib import Path
import shutil
import select
import subprocess
import sys
import tempfile

SCRIPTS = Path(__file__).resolve().parent


def run(*args):
    return subprocess.run(args, text=True, capture_output=True, check=True)


def start_ready(executable, probe):
    child = subprocess.Popen([str(executable)], stdout=subprocess.PIPE, text=True)
    try:
        ready, _, _ = select.select([child.stdout], [], [], 10)
        assert ready and child.stdout.readline().strip() == "ready", \
            f"test app did not finish launching (exit {child.poll()})"
        assert run(str(probe), str(child.pid)).stdout.strip() == "0", \
            "test app signature was invalid before replacement"
        return child
    except BaseException:
        child.terminate()
        child.wait(timeout=5)
        raise


def main():
    if sys.platform != "darwin":
        raise SystemExit("This regression test requires macOS")
    with tempfile.TemporaryDirectory(prefix="del-install-test-") as temporary:
        root = Path(temporary)
        probe_source = root / "probe.c"
        probe_source.write_text('''
#include <Security/Security.h>
#include <CoreFoundation/CoreFoundation.h>
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    int pid = atoi(argv[1]);
    CFNumberRef number = CFNumberCreate(NULL, kCFNumberIntType, &pid);
    const void *keys[] = { kSecGuestAttributePid };
    const void *values[] = { number };
    CFDictionaryRef attributes = CFDictionaryCreate(NULL, keys, values, 1,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    SecCodeRef code = NULL;
    OSStatus status = SecCodeCopyGuestWithAttributes(NULL, attributes, 0, &code);
    if (status == 0) status = SecCodeCheckValidity(code, 0, NULL);
    printf("%d\\n", (int)status);
    return 0;
}
''')
        probe = root / "probe"
        run("/usr/bin/clang", str(probe_source), "-framework", "Security",
            "-framework", "CoreFoundation", "-o", str(probe))
        identity = root / "identity"
        source = root / "main.c"
        source.write_text("#include <unistd.h>\n#include <stdio.h>\n"
                          'int main(void) { puts("ready"); fflush(stdout); for (;;) pause(); }\n')
        bundle = root / "staged.app"
        executable = bundle / "Contents/MacOS/del"
        executable.parent.mkdir(parents=True)
        shutil.copy(SCRIPTS.parent / "crates/app/Info.plist", bundle / "Contents/Info.plist")
        run("/usr/bin/clang", str(source), "-o", str(executable))
        run(sys.executable, str(SCRIPTS / "sign-macos.py"), str(bundle),
            "--identity-directory", str(identity))
        child = None
        try:
            # Control: deleting a live bundle reproduces the original failure.
            deleted = root / "deleted.app"
            shutil.copytree(bundle, deleted)
            child = start_ready(deleted / "Contents/MacOS/del", probe)
            shutil.rmtree(deleted)
            assert run(str(probe), str(child.pid)).stdout.strip() != "0"
            child.terminate()
            child.wait(timeout=5)
            child = None

            installed = root / "installed/del.app"
            run(sys.executable, str(SCRIPTS / "install-macos.py"), str(bundle), str(installed))
            child = start_ready(installed / "Contents/MacOS/del", probe)
            for _ in range(2):
                shutil.copytree(installed, bundle)
                run(sys.executable, str(SCRIPTS / "install-macos.py"), str(bundle), str(installed))
                status = run(str(probe), str(child.pid)).stdout.strip()
                assert status == "0", (
                    f"replacement invalidated the running process's signature: {status}; "
                    f"process exit: {child.poll()}; "
                    f"retained: {list((installed.parent / '.del-retired').glob('*.app'))}; "
                    f"open files: {subprocess.run(['/usr/sbin/lsof', '-a', '-p', str(child.pid), '-d', 'txt', '-Fn'], capture_output=True, text=True).stdout}"
                )
            retired = installed.parent / ".del-retired"
            assert len(list(retired.glob("*.app"))) == 1
            child.terminate()
            child.wait(timeout=5)
            child = None
            shutil.copytree(installed, bundle)
            run(sys.executable, str(SCRIPTS / "install-macos.py"), str(bundle), str(installed))
            assert not list(retired.glob("*.app")), "exited bundles were not cleaned up"
            print("PASS: deletion reproduces identity failure; live replacements preserve identity; exited bundles are cleaned up")
        finally:
            if child is not None:
                child.terminate()
                child.wait(timeout=5)
            run("/usr/bin/security", "delete-keychain", str(identity / "identity.keychain-db"))


if __name__ == "__main__":
    main()
