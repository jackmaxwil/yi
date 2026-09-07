import re

NON_WORD = re.compile(r"[^a-z]+")


def slugify(text):
    return NON_WORD.sub("-", text.lower()).strip("-")
