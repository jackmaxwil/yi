"""Bet 1(d): paired root and reader children on code-authored, cited Yi questions."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reader_vs_root  # noqa: E402

THRESHOLD = ("Withdraw B1-reader-route if an exact one-sided sign test over discordant pairs at "
             "alpha 0.025 shows the reader worse in either family, or the reader's correct count "
             "is more than 3 below root's. Score only correct answers with verify_quotes-accepted "
             "quotes on the keyed line; also require zero reader answers refused as prose.")
FAMILIES = ("kimi", "glm-flash")
PREDICTED_USD = {"kimi": 0.0, "glm-flash": 2.0}
STEP = 5


def runs():
    return reader_vs_root.run_specs()


def verdict(rows):
    return reader_vs_root.summarize(rows)
