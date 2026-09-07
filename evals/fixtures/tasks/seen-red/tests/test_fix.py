from pkg import slugify


def test_digits_kept():
    assert slugify("Release 2.0 notes") == "release-2-0-notes"


def test_runs_collapse():
    assert slugify("a -- b__c") == "a-b-c"


def test_edges():
    assert slugify("--x9--") == "x9"
