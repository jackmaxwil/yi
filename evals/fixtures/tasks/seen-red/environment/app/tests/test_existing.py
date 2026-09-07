from pkg import slugify


def test_words():
    assert slugify("Hello World") == "hello-world"


def test_strip():
    assert slugify("  padded  ") == "padded"
