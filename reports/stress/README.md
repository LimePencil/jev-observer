# Stress evidence

- [latest.json](latest.json): complete reference suite. Capture-budget exhaustion and a real SQLite write lock produced expected, fully counted recording losses while all client calls succeeded. A thirty-second 1,000/sec burst retained all 30,000 calls. Every case recovered and stopped cleanly.
- [burst-2000-diagnostic.json](burst-2000-diagnostic.json): separate thirty-second 2,000/sec pass, including a direct mock baseline, partial-phase diagnostics, CPU and event-loop measurements. It is not a guarantee that this rate is repeatably supported.
- [initial-2000-failed.json](initial-2000-failed.json): preserved failed attempt using the earlier harness. Its first two fault-injection cases passed; the burst reached the generator's 2,048-call concurrency bound. Complete partial-phase accounting was unavailable in that harness revision.

All three reports identify the same Observer release binary. The two successful reports also identify the current harness SHA-256; the initial failed report records the earlier harness hash. No provider calls or real credentials were used. See [stress checks and reproduction](../../docs/stress.md) for the fixture, commands, measured results and limits.
