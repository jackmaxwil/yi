# Journey prompts

Ten prompts, two per request class plus one monitor and one ambiguous, run
against a real model under two prompt refs (A and B) and scored by
`skills/yi/session-mining/extract.py` over the session files each run
writes. User-run, never scheduled (flywheel law 3); CI keeps the faux tier.

    python3 evals/journeys/ab.py --model openrouter/z-ai/glm-5.3-flash --ref A --binary ./target/debug/yi --cwd .

One line per prompt in `prompts.txt`; the class is the first word, the
prompt the rest. The runner writes one JSONL per prompt under
`--out/<ref>/` and prints the extractor's signals table for the run; a
human reads the final answers against the Voice rows.
