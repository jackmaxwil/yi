---
{
  "format": 1,
  "plan": "ship-logrotate-lite-with-a-packaged",
  "goal": "Ship logrotate-lite with a packaged tarball",
  "version": 2,
  "touched": 21,
  "tier": "root",
  "spawns": 2,
  "state": "done",
  "todos": [
    {
      "label": "Write the Makefile dist target",
      "state": "done"
    },
    {
      "label": "Drive the whole build through make dist",
      "state": "done",
      "output": "local://dist/logrotate-lite.tar.gz",
      "after": [
        "Write the Makefile dist target"
      ]
    }
  ]
}
---
