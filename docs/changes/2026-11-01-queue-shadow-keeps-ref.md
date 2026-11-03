---
version: 0.885.0
issue: Refs #1966
---
A shadow tick that finds its candidate already in main now keeps the ref, so the next `on` tick still closes the PR's issues and clears its label.
