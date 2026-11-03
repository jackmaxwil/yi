import subprocess
import sys

subprocess.run(f"printf {sys.argv[1]}", shell=False, check=True)
