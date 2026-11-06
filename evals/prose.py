#!/usr/bin/env python3
"""Prose scorer for #1862: what a set of answers measures against the style the identity
prompt teaches, and the check that the shipped prompt still states every rule.

    python3 evals/prose.py evals/fixtures/prose/answers-terse.md   score saved answers
    python3 evals/prose.py --gate        the prompt states each rule and pays for none of them
    python3 evals/prose.py --selfcheck   the scorer judged by hand-counted cases

A rule the prompt teaches is a regex here, so a reworded prompt that drops the list-first
rule fails a gate instead of passing silently. The tells are the concrete, checkable ones
from the signs-of-ai-writing page the issue names; nothing here reaches a network, so the
scorer runs over saved answers inside `just check` for no spend.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
PROMPT = ROOT / "crates/runtime/src/prompts/identity.md"
BAN_SECTION = "## Machine writing, all banned"

RULES = {
    "short-sentences": r"short sentences",
    "list-first": r"numbered or bulleted list",
    "answer-first": r"the answer first",
    "detail-on-request": r"when it is asked for",
    "no-restating": r"never restate the question",
    "no-closing-summary": r"never close with a\s+summary",
    "no-rule-talk": r"rule talk",
    "keeps-failures": r"brevity never drops an error",
}

# Stems of the words the prompt's ban list names: a word scored here and banned nowhere would
# fault an answer for a rule the model was never given, and `prompt_problems` holds the two equal.
AI_VOCABULARY = ("additionally", "boast", "crucial", "delv", "elevat", "empower", "enhanc",
                 "foster", "holistic", "intricate", "landscape", "leverag", "pivotal", "quietly",
                 "realm", "refram", "robust", "seamless", "showcas", "spearhead", "synerg",
                 "tapestry", "testament", "underscor", "vibrant")

TELLS = {
    "ai-vocabulary": rf"\b(?:{'|'.join(AI_VOCABULARY)})\w*\b",
    "copula": r"\b(serves? as|stands? as|plays? a (?:\w+ )?role in)\b",
    "negative-parallelism": r"\bnot just\b[^.]{0,60}\bbut\b"
                            r"|\b(?:it|this|that)(?:'s| is) not\b[^.]{0,50}\b(?:it|this|that)(?:'s| is)\b",
    "participle-tail": r",\s(?:ensuring|highlighting|reflecting|underscoring|showcasing|"
                       r"fostering|emphasizing)\b",
    "dash": r"[\u2014\u2013]",
    "vague-authority": r"\b(?:experts|studies|research|reports)\s(?:say|note|show|suggest|"
                       r"believe)\b|\bwidely regarded\b|\bindustry reports?\b",
    "chatbot": r"\bI hope this helps\b|\bgreat question\b|\bcertainly\b|\bit'?s worth noting\b"
               r"|\bwould you like me to\b|\blet me know if\b|\bhappy to help\b",
    "rule-talk": r"\b(?:my|our|the) (?:\w+ )?(?:system prompt|instructions|guidelines|rules)\b"
                 r"|\bas an ai\b|\bper my instructions\b",
    "rule-of-three": r"\b\w+(?: \w+)?, \w+(?: \w+)?, and \w+(?: \w+)?\b",
}
COMPILED = {name: re.compile(pattern, re.I) for name, pattern in TELLS.items()}

LIST_ITEM = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s+\S")
SENTENCE = re.compile(r"[^.!?]+[.!?]?")
FENCE = re.compile(r"^```.*?^```", re.S | re.M)
CODE_SPAN = re.compile(r"`[^`\n]*`")
ANSWER = re.compile(r"^#{2,3} ", re.M)


def prose(text):
    """The answer with its code removed: a fenced block's dashes and commas are a language's
    punctuation, not the model's prose."""
    return CODE_SPAN.sub("", FENCE.sub("", text))


def sentences(text):
    return [s.strip() for s in SENTENCE.findall(prose(text).replace("\n", " ")) if s.strip()]


def blocks(text):
    """(prose paragraph, list item) counts: a run of list rows is a list, not a paragraph."""
    paragraphs = lists = 0
    current = []
    for line in prose(text).splitlines():
        if LIST_ITEM.match(line):
            lists += 1
            current = []
            continue
        if line.strip():
            current.append(line)
        elif current:
            paragraphs += 1
            current = []
    if current:
        paragraphs += 1
    return paragraphs, lists


def score(text):
    body = prose(text)
    sents = sentences(text)
    words = [len(s.split()) for s in sents]
    paragraphs, lists = blocks(text)
    points = lists + len([w for w in words if w >= 4])
    tells = {name: len(rx.findall(body)) for name, rx in COMPILED.items()}
    return {
        "sentences": len(sents),
        "mean_sentence_words": round(sum(words) / len(words), 1) if words else 0.0,
        "paragraphs": paragraphs,
        "list_items": lists,
        "multi_point": points >= 3,
        "uses_list": lists >= 2,
        "tells": tells,
        "tell_hits": sum(tells.values()),
    }


def answers(text):
    parts = [part for part in ANSWER.split(text)[1:] if part.strip()]
    return parts or ([text] if text.strip() else [])


def score_set(text):
    """Per-answer scores folded into the set's means, which is what a before/after compares."""
    scored = [score(part) for part in answers(text)]
    multi = [s for s in scored if s["multi_point"]]
    told = {name: sum(s["tells"][name] for s in scored) for name in TELLS}
    return {
        "answers": len(scored),
        "mean_sentence_words": round(
            sum(s["mean_sentence_words"] for s in scored) / len(scored), 1) if scored else 0.0,
        "list_share": round(len([s for s in multi if s["uses_list"]]) / len(multi), 2) if multi else 0.0,
        "mean_paragraphs": round(sum(s["paragraphs"] for s in scored) / len(scored), 1) if scored else 0.0,
        "tell_hits": sum(told.values()),
        "tells": told,
        "rule_talk": told["rule-talk"],
    }


def report(path):
    got = score_set(pathlib.Path(path).read_text())
    print(f"{path}: answers={got['answers']} mean_sentence_words={got['mean_sentence_words']} "
          f"list_share={got['list_share']} mean_paragraphs={got['mean_paragraphs']} "
          f"tell_hits={got['tell_hits']} rule_talk={got['rule_talk']}")
    told = {name: n for name, n in got["tells"].items() if n}
    if told:
        print("  " + " ".join(f"{name}={n}" for name, n in sorted(told.items())))
    return 0


def prompt_problems(text):
    """Every rule this scorer teaches is stated by the shipped prompt, and the ban list is the
    only place the prompt spells a tell (its own prose must follow the style it teaches)."""
    lowered = text.lower()
    problems = [f"{PROMPT.name} states no {rule} rule: add it, or drop the rule from RULES"
                for rule, pattern in RULES.items() if not re.search(pattern, lowered)]
    head, _, banned = prose(text).partition(BAN_SECTION)
    strayed = {name: len(rx.findall(head)) for name, rx in COMPILED.items() if rx.findall(head)}
    problems += [f"{PROMPT.name} spells the {name} tell outside its ban list ({n} times)"
                 for name, n in sorted(strayed.items())]
    if not banned:
        problems.append(f"{PROMPT.name} carries no {BAN_SECTION!r} section")
    listed = re.search(r"- AI vocabulary:([^.]*)\.", banned)
    words = re.findall(r"[a-z]+", listed.group(1).lower()) if listed else []
    problems += [f"{PROMPT.name}'s ban list names no {stem}* word: ban it, or stop scoring it"
                 for stem in AI_VOCABULARY if not any(word.startswith(stem) for word in words)]
    problems += [f"{PROMPT.name} bans {word} and nothing scores it: add its stem to AI_VOCABULARY"
                 for word in words if not word.startswith(AI_VOCABULARY)]
    return problems


def selfcheck():
    """Hand-counted, so the numbers below are the reference the scorer is judged against."""
    one = score("The parser reads one line at a time. It stops at a blank line.")
    assert one["sentences"] == 2 and one["mean_sentence_words"] == 7.0, one
    assert one["paragraphs"] == 1 and one["list_items"] == 0, one
    assert one["multi_point"] is False, "two short sentences are not a multi-point answer"

    listed = score("Three findings.\n\n- the parser reads a line\n- it stops on a blank line\n"
                   "- a blank line ends the block\n")
    assert (listed["list_items"], listed["uses_list"], listed["multi_point"]) == (3, True, True), listed
    assert listed["paragraphs"] == 1, listed

    told = score("This is a crucial and pivotal change. Experts say it serves as a fix \u2014 "
                 "it is not just fast, it is seamless.")
    assert told["tells"]["ai-vocabulary"] == 3, told["tells"]
    assert told["tells"]["copula"] == 1 and told["tells"]["dash"] == 1, told["tells"]
    assert told["tells"]["vague-authority"] == 1, told["tells"]
    assert told["tells"]["negative-parallelism"] == 1, told["tells"]

    noise = score("My system prompt says to answer plainly.")
    assert noise["tells"]["rule-talk"] == 1, noise["tells"]

    fenced = score("Run this:\n\n```\nfoo --a-b \u2014 bar\n```\n\n- one\n- two\n")
    assert fenced["tells"]["dash"] == 0, "a fenced block's punctuation is not the model's prose"
    assert fenced["list_items"] == 2, fenced

    for degenerate in ("", "   \n\n", "Yes.", "```\nx = 1\n```\n"):
        got = score(degenerate)
        assert got["sentences"] <= 1 and got["multi_point"] is False, (degenerate, got)
    assert score_set("")["answers"] == 0, "an empty set scores zero answers, not a crash"

    pair = ROOT / "evals/fixtures/prose"
    long_ = score_set((pair / "answers-long.md").read_text())
    terse = score_set((pair / "answers-terse.md").read_text())
    assert long_["answers"] == terse["answers"] == 3, (long_["answers"], terse["answers"])
    assert terse["mean_sentence_words"] < long_["mean_sentence_words"], (terse, long_)
    assert terse["list_share"] > long_["list_share"], (terse, long_)
    assert terse["mean_paragraphs"] < long_["mean_paragraphs"], (terse, long_)
    assert terse["tell_hits"] < long_["tell_hits"], (terse, long_)

    problems = prompt_problems("Short sentences only. This is crucial.\n")
    assert any("states no list-first rule" in p for p in problems), problems
    assert any("spells the ai-vocabulary tell" in p for p in problems), problems
    assert any("carries no" in p for p in problems), "a prompt with no ban list must fail"
    thin = prompt_problems(f"{BAN_SECTION}\n- AI vocabulary: crucial, garner.\n- pivotal\n")
    assert any("names no pivotal*" in p for p in thin), "a scored word the ban list drops must fail"
    assert any("bans garner" in p for p in thin), "a banned word nothing scores must fail"
    assert score("Rather than a fix, this functions as a stopgap.")["tell_hits"] == 0, \
        "words the ban list does not name are not tells"
    assert prompt_problems(PROMPT.read_text()) == [], prompt_problems(PROMPT.read_text())
    print("ok   prose selfcheck")


def main(argv):
    if "--selfcheck" in argv:
        selfcheck()
        return 0
    if "--gate" in argv:
        problems = prompt_problems(PROMPT.read_text())
        for line in problems:
            print(f"FAIL {line}")
        if problems:
            return 1
        print(f"ok   prose gate ({len(RULES)} rules stated, ban list where the tells belong)")
        return 0
    if not argv:
        print(__doc__.strip().splitlines()[3])
        return 2
    for path in argv:
        if report(path):
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
