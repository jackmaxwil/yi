#!/usr/bin/env python3
"""scripts/sign.sh signs with the release key by default, names the fix when it cannot, and allowed_signers is well formed."""
import base64
import hashlib
import os
import pathlib
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from intake import bridge

SIGN = ROOT / "scripts/sign.sh"
SEED = bytes.fromhex("62855843e5676bbe1486e1d75d8809f70ae78fd9af386d9c128f0c50204de402")
PUBLIC = bytes.fromhex("24bec907db51fab87fc5614d0929b6a6038ab93f4c770958231f58cc1c44ccd2")
OWNER_RELEASE_KEY = "SHA256:Tb4DlV9Kzm6hIEEnTNTjGNPv4msLyf4zlFPIZZlpsOE"


def keygen_probe():
    """Whether this host can make a key at all: a uid with no passwd entry makes ssh-keygen exit 255."""
    with tempfile.TemporaryDirectory() as tmp:
        made = subprocess.run(("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "probe", "-f", str(pathlib.Path(tmp) / "k")),
                              capture_output=True, text=True, stdin=subprocess.DEVNULL)
    return made.returncode == 0, made.stderr.strip() or "no key was written"


def fingerprint(signers_line):
    """The SHA256 fingerprint ssh-keygen prints for an allowed_signers line, computed without ssh-keygen."""
    key = signers_line.split()[-1]
    blob = base64.b64decode(key + "=" * (-len(key) % 4))
    return "SHA256:" + base64.b64encode(hashlib.sha256(blob).digest()).decode().rstrip("=")


CAN_KEYGEN, KEYGEN_TROUBLE = keygen_probe()
needs_keygen = unittest.skipUnless(CAN_KEYGEN, f"this host cannot make the key fixtures, because ssh-keygen fails: {KEYGEN_TROUBLE}")


def run(*args, env=None, check=True, stdin_text=None):
    # No controlling terminal: a passphrase prompt cannot be answered, as in a script or CI.
    return subprocess.run(args, capture_output=True, text=True, env=env, check=check, input=stdin_text,
                          stdin=None if stdin_text is not None else subprocess.DEVNULL, start_new_session=True)


def passwd_env(directory):
    # Invariant: macOS has no getent, and resolves the uid through Directory Services already.
    if not shutil.which("getent") or subprocess.run(["getent", "passwd", str(os.getuid())], capture_output=True).returncode == 0:
        return {}
    wrapper = directory / "passwd.so"
    source = """#include <pwd.h>
#include <sys/types.h>
#include <unistd.h>
struct passwd *getpwuid(uid_t uid) {
    static struct passwd user = {"yi", "x", 0, 0, "", "/tmp", "/bin/sh"};
    user.pw_uid = uid;
    user.pw_gid = getgid();
    return &user;
}
"""
    compiled = subprocess.run(["cc", "-shared", "-fPIC", "-x", "c", "-", "-o", str(wrapper)], input=source, capture_output=True, text=True)
    if compiled.returncode != 0:
        raise RuntimeError(compiled.stderr)
    return {"LD_PRELOAD": str(wrapper)}


def field(data):
    return struct.pack(">I", len(data)) + data


def fixture(directory, key):
    blob = field(b"ssh-ed25519") + field(PUBLIC)
    pathlib.Path(f"{key}.pub").write_text("ssh-ed25519 " + base64.b64encode(blob).decode() + " fixture\n")
    sealed = struct.pack(">II", int.from_bytes(b"yi", "big"), int.from_bytes(b"yi", "big"))
    private = sealed + field(b"ssh-ed25519") + field(PUBLIC) + field(SEED + PUBLIC) + field(b"fixture")
    private += bytes(range(1, 9 - len(private) % 8))
    body = b"openssh-key-v1\0" + field(b"none") + field(b"none") + field(b"") + struct.pack(">I", 1) + field(blob) + field(private)
    wrapped = base64.b64encode(body).decode()
    key.write_text(f"-----BEGIN OPENSSH " + "PRIVATE KEY-----\n" + "\n".join(wrapped[at:at + 70] for at in range(0, len(wrapped), 70)) + "\n-----END OPENSSH " + "PRIVATE KEY-----\n")
    key.chmod(0o600)
    (directory / "signers").write_text(f"signer {pathlib.Path(f'{key}.pub').read_text().strip()}\n")
    (directory / "a.tar.gz").write_text("release bytes")
    return key


@needs_keygen
class SignWithAKey(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.key = fixture(self.dir, self.dir / "key")
        self.env = {**os.environ, **passwd_env(self.dir), "YI_SIGNING_KEY": str(self.key), "YI_ALLOWED_SIGNERS": str(self.dir / "signers")}
        self.env.pop("SSH_ASKPASS", None)
        self.env.pop("SSH_AUTH_SOCK", None)

    def tearDown(self):
        self.tmp.cleanup()

    def test_a_key_it_cannot_read_fails_and_names_the_fix(self):
        unreadable = self.dir / "unreadable"
        unreadable.write_text(pathlib.Path(f"{self.key}.pub").read_text())
        env = {**self.env, "YI_SIGNING_KEY": str(unreadable)}
        signed = run(str(SIGN), str(self.dir / "a.tar.gz"), env=env, check=False)
        self.assertNotEqual(signed.returncode, 0)
        self.assertIn("ssh-add", signed.stderr)
        self.assertFalse((self.dir / "a.tar.gz.sig").exists())


class TrustList(unittest.TestCase):
    def signer_lines(self):
        return [line for line in (ROOT / "allowed_signers").read_text().splitlines() if line.strip() and not line.startswith("#")]

    def test_allowed_signers_is_the_owners_release_key_alone(self):
        lines = self.signer_lines()
        self.assertEqual(len(lines), 1, "the owner is the only signer")
        computed = fingerprint(lines[0])
        self.assertEqual(computed, OWNER_RELEASE_KEY, "changing the trusted key is a deliberate edit of this line")
        keytype, body = lines[0].split()[1:3]
        self.assertEqual(keytype, "ssh-ed25519", "the release key is an ed25519 key")
        blob = base64.b64decode(body)
        self.assertEqual(blob[4:4 + struct.unpack(">I", blob[:4])[0]].decode(), keytype, "the keytype is the one the blob declares")
        self.assertEqual(bridge.key_fingerprint(lines[0].split(None, 1)[1]), computed, "the intake parser reads the release key fingerprint")

    def test_a_trailing_comment_stays_out_of_the_hashed_key(self):
        line = self.signer_lines()[0]
        self.assertEqual(bridge.key_fingerprint(line.split(None, 1)[1] + " -C yi-release"), OWNER_RELEASE_KEY, "sign.sh writes the .pub, comment and all")

    def test_a_keytype_the_blob_does_not_declare_is_refused(self):
        key = self.signer_lines()[0].split(None, 1)[1]
        with self.assertRaises(ValueError):
            bridge.key_fingerprint("ssh-rsa" + key[key.index(" "):])

    @needs_keygen
    def test_the_default_key_is_the_release_key(self):
        with tempfile.TemporaryDirectory() as home:
            home = pathlib.Path(home)
            (home / ".ssh").mkdir()
            fixture(home, home / ".ssh" / "yi-release")
            env = {k: v for k, v in os.environ.items() if k != "YI_SIGNING_KEY"}
            env.update(passwd_env(home), HOME=str(home), YI_ALLOWED_SIGNERS=str(home / "signers"))
            signed = run(str(SIGN), str(home / "a.tar.gz"), env=env, check=False)
            self.assertEqual(signed.returncode, 0, signed.stdout + signed.stderr)
            self.assertTrue((home / "a.tar.gz.sig").is_file())


if __name__ == "__main__":
    unittest.main()
