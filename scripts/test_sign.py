#!/usr/bin/env python3
"""scripts/sign.sh signs with the release key by default, names the fix when it cannot, and allowed_signers is well formed."""
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
SIGN = ROOT / "scripts/sign.sh"
PASSPHRASE = "correct horse"
# The public fingerprint of the owner's release key (~/.ssh/yi-release): the one signer yi update trusts.
OWNER_RELEASE_KEY = "SHA256:lNXKild9bEMo3z1To8V08WGUGJvag3cLRasaxyGVawo"


def run(*args, env=None, check=True, stdin_text=None):
    # No controlling terminal: a passphrase prompt cannot be answered, as in a script or CI.
    return subprocess.run(args, capture_output=True, text=True, env=env, check=check, input=stdin_text,
                          stdin=None if stdin_text is not None else subprocess.DEVNULL, start_new_session=True)


def fixture(directory, key, passphrase):
    """A release file, a key and the allowed_signers line that trusts it, all under `directory`."""
    run("ssh-keygen", "-q", "-t", "ed25519", "-N", passphrase, "-C", "t", "-f", str(key))
    (directory / "signers").write_text(f"signer {pathlib.Path(f'{key}.pub').read_text().strip()}\n")
    (directory / "a.tar.gz").write_text("release bytes")


class SignWithAKey(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self.tmp.name)
        self.key = self.dir / "key"
        fixture(self.dir, self.key, PASSPHRASE)
        self.env = {**os.environ, "YI_SIGNING_KEY": str(self.key), "YI_ALLOWED_SIGNERS": str(self.dir / "signers")}
        self.env.pop("SSH_ASKPASS", None)
        self.env.pop("SSH_AUTH_SOCK", None)

    def tearDown(self):
        self.tmp.cleanup()

    def test_a_protected_key_that_cannot_be_read_fails_and_names_the_fix(self):
        signed = run(str(SIGN), str(self.dir / "a.tar.gz"), env=self.env, check=False)
        self.assertNotEqual(signed.returncode, 0)
        self.assertIn("ssh-add", signed.stderr)
        self.assertFalse((self.dir / "a.tar.gz.sig").exists())


class TrustList(unittest.TestCase):
    def test_allowed_signers_is_the_owners_release_key_alone(self):
        lines = [line for line in (ROOT / "allowed_signers").read_text().splitlines() if line.strip() and not line.startswith("#")]
        self.assertEqual(len(lines), 1, "the owner is the only signer")
        keyed = run("ssh-keygen", "-l", "-f", "-", check=False, stdin_text=lines[0].split(None, 1)[1])
        self.assertEqual(keyed.returncode, 0, keyed.stderr)
        self.assertIn(OWNER_RELEASE_KEY, keyed.stdout, "changing the trusted key is a deliberate edit of this line")

    def test_the_default_key_is_the_release_key(self):
        with tempfile.TemporaryDirectory() as home:
            home = pathlib.Path(home)
            (home / ".ssh").mkdir()
            fixture(home, home / ".ssh" / "yi-release", "")
            env = {k: v for k, v in os.environ.items() if k != "YI_SIGNING_KEY"}
            env.update(HOME=str(home), YI_ALLOWED_SIGNERS=str(home / "signers"))
            signed = run(str(SIGN), str(home / "a.tar.gz"), env=env, check=False)
            self.assertEqual(signed.returncode, 0, signed.stdout + signed.stderr)
            self.assertTrue((home / "a.tar.gz.sig").is_file())


if __name__ == "__main__":
    unittest.main()
