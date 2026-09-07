from . import core

OPS = {"+": core.add, "-": core.subtract, "*": core.multiply, "/": core.divide}


def parse(text):
    left, op, right = text.split(" ")
    return OPS[op](float(left), float(right))
