import sys


def greet(name):
    message = "Hello, " + name
    print(message)
    return message


def main(argv):
    who = argv[1] if len(argv) > 1 else "world"
    greet(who)


if __name__ == "__main__":
    main(sys.argv)
