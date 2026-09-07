import calc


def test_add():
    assert calc.add(2, 3) == 5


def test_parse_spaced():
    assert calc.parse("3 + 4") == 7
