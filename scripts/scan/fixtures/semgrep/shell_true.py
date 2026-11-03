import subprocess

command = str("printf ok")
subprocess.run(command, shell=True, check=True)
