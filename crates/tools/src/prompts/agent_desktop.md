# agent-desktop

`agent-desktop` reads and operates Mac apps through their accessibility trees. Run it by that
plain name, alone: nothing piped, no wrapper. You are asked once per app; browsers are refused, and so is whatever takes the user's own mouse,
keyboard or front app, since they are working beside you.

    agent-desktop session start --cursor   # your own visible cursor; pass --session <id> below
    agent-desktop snapshot --app TextEdit -i --compact --session <id>   # refs like @s8f3k2p9:e5
    agent-desktop click @s8f3k2p9:e5 --session <id>   # then snapshot again to check

Read `yi://skills/desktop` before driving an app.
