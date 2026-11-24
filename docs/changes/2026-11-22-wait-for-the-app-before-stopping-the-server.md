---
version: 0.1101.0
issue: Refs #2257
---
Yi Desktop's two journeys that stop the server while the app is open now wait until the app has really connected. The runner's count of the server's connections was never zero, so the server was stopped two seconds after the app was launched whether it had connected or not, and on a slow start the app never drew Reconnect.
