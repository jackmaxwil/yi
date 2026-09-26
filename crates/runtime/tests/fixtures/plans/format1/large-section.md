---
{
  "format": 1,
  "plan": "vendor-the-zstd-backend",
  "goal": "Vendor the zstd backend and document the rotation contract",
  "version": 1,
  "touched": 3,
  "tier": "root",
  "spawns": 1,
  "state": "active",
  "todos": [
    {
      "label": "Write the rotation contract note",
      "state": "pending",
      "delegation": {
        "spec": {
          "role": "writer",
          "effort": "low"
        },
        "accept": {
          "stated": "the note names every trigger and every failure mode"
        },
        "context": [
          "local://README.md"
        ]
      }
    },
    {
      "label": "Vendor zstd and pin its version",
      "state": "running",
      "by": "vendor-child-1",
      "after": [
        "Write the rotation contract note"
      ],
      "delegation": {
        "spec": {
          "role": "coder",
          "effort": "med",
          "isolation": "worktree"
        },
        "accept": {
          "command": "pytest -q tests/test_vendor.py"
        },
        "output": {
          "schema": "local://.yi/schemas/test_report.json"
        },
        "context": [
          "plan://vendor-the-zstd-backend/write-the-rotation-contract-note"
        ]
      }
    }
  ]
}
---
## Write the rotation contract note
Paragraph 1. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 2. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 3. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 4. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 5. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 6. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 7. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 8. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 9. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 10. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 11. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 12. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 13. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 14. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 15. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 16. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 17. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 18. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 19. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Paragraph 20. The rotation contract is what a reader of the log directory can rely on without reading the source. It names the triggers, the ordering between them, the file the writer keeps open across a rotation, and the failure modes an operator has to be able to tell apart from a healthy run.

Padding so the section is exactly six kibibytes: vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv
