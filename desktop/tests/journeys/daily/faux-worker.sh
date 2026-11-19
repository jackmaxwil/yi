#!/bin/sh
# A worker binary for `_yi/worker_replace`: the real `yi acp`, fed a reply cassette, since `yi serve` refuses --faux.
exec yi "$@" --faux "$(dirname "$0")/cassette.jsonl"
