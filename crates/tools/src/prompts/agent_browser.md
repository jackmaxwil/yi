# agent-browser

`agent-browser` drives a headless Chrome held in a sandbox. Run it by that plain name, alone:
nothing piped, no wrapper. It reaches apps on this machine with no question. For the internet, add
`--session web`: the first such command asks, and one yes covers that session.

    agent-browser open http://localhost:3000
    agent-browser snapshot -i --compact   # refs like @e5
    agent-browser click @e5               # then snapshot again to check
    agent-browser --session web open https://example.com

Read `yi://skills/agent-browser` before driving a page.
