# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Edit the right-hand column to match whatever vocabulary you actually use.

## Labels beside the triage roles

These are not triage roles; apply them alongside one whenever an issue is filed or triaged.

| Label | Apply when |
| ----- | ---------- |
| `windows` | The issue has to be worked on and verified on Windows. |
| `macos` | The issue has to be worked on and verified on macOS. |
| `spec` | The issue is a contract that work issues are cut from, not a work item itself. |
| `bug` / `enhancement` | The issue's type: something that works wrongly, or something new. |

A platform label says which machine the work needs, not which platform the code talks about. An issue that needs both machines takes both labels. Behaviour reproduced only through the CDP engine is `windows` until macOS has an engine that can reproduce it.
