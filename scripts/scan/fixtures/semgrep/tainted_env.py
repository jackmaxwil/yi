import os
import subprocess

subprocess.run(os.environ["COMMAND"], shell=False, check=True)
