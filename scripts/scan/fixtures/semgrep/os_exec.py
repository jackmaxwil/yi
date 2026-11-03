import os

os.execv("/bin/sh", ["sh", "-c", os.environ["COMMAND"]])
