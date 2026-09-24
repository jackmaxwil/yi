import os
import sys

sys.path.insert(0, os.environ.get("APP", "/app"))

from numkit import median
from textkit import wrap


def test_median_of_an_odd_list():
    assert median([3, 1, 2]) == 2


def test_wrap_breaks_at_a_word_boundary():
    assert wrap("aa bb cc", 5).split("\n")[0] == "aa bb"
