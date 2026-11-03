import hashlib
import subprocess

subprocess.run(["printf", "ok"], shell=False, check=True)
hashlib.md5(b"fixture", usedforsecurity=False)
