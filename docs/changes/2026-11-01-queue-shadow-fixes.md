---
issue: Refs #1966
---
Shadow no longer writes main and announces each PR head once, so it stops gating the oldest PR forever and reaches the rest of the queue. Under `on`, a recorder push refused for any reason but a moved main no longer stops the queue from building.
