import sys

import pytest

import os
sys.path.insert(0, os.environ.get("APP", "/app"))
import calc  # noqa: E402


def test_1_divide_raises():
    with pytest.raises(ZeroDivisionError, match="divide by zero"):
        calc.divide(1, 0)


def test_2_power():
    assert calc.power(2, 10) == 1024


def test_3_parse_whitespace():
    assert calc.parse("3+4") == 7
    assert calc.parse("3   +   4") == 7


def test_4_parse_caret():
    assert calc.parse("2 ^ 3") == 8


def test_5_export_and_version():
    assert "power" in calc.__all__
    assert calc.__version__ == "0.2.0"


def test_existing_stay_green():
    assert calc.add(2, 3) == 5 and calc.parse("3 + 4") == 7
