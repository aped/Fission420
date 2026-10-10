# Kickoff prompt

Copy the text below into your coding agent after placing this pack at the repository root and editing GOAL.md. This starts implementation in that coding session; it is not a scheduler.

---

Read AGENTS.md, GOAL.md, PLAN.md, and plan/STATUS.md. Implement the goal in this repository now. Work autonomously through the bounded run; do not stop after a plan or scaffold.

You are authorized to inspect the repo, make project-local changes, resolve approved dependencies, build, test, run the application, and make local checkpoint commits on a work branch. Preserve existing user work. Do not push or publish. Follow the permission and dependency boundaries in AGENTS.md.

Record the actual start time and deadline once. The defaults are six hours, at most 24 bounded implementation tasks, and a final 30-minute integration reserve. Continue until the acceptance goal is met, the budget expires, or no authorized useful work remains. Do not wait artificially. These are upper bounds, not instructions to exhaust tokens or money.

Start with repository/environment inspection and a time-boxed geometry-kernel spike. Use current official API documentation and compiled experiments, not remembered APIs. Make a documented kernel choice, then build the smallest complete headless parametric workflow before connecting the native UI. Keep a working example throughout development.

Make ordinary decisions yourself and document material assumptions. If an optional dependency, reference application, display server, network route, or approval is unavailable, record the limitation and keep working on independent tasks. Never bypass access controls or fabricate successful checks.

You may delegate disjoint implementation or review tasks up to the worker count in GOAL.md if your runtime supports it. Establish shared interfaces first; one coordinator owns integration. Use serial work if delegation is unavailable. Keep all workers within this run's deadline and task budget.

Update plan/STATUS.md after each feature arc so a new session can resume without reconstructing context. Validate geometry with independent expected values, test upstream edits and rollback, exercise the actual UI if available, and record evidence. Screenshots and a compiled binary alone do not establish completion.

Finish with a last working checkpoint and MORNING_REPORT.md: what runs, actual commands and results, a five-minute demo, evidence paths, known limitations, and the next three tasks. If full success is blocked, deliver the strongest verified partial result and state exactly which acceptance cases remain open.

Begin now.

---

## Resume prompt

Read AGENTS.md, GOAL.md, PLAN.md, plan/STATUS.md, and MORNING_REPORT.md if present. Inspect the current working tree and verify the recorded baseline. Resume the recorded next task within the original remaining budget; do not reset the deadline or restart completed research. If that budget has expired, finish the handoff and wait for a new run budget.

## Start a second run

Read the previous MORNING_REPORT.md and current status. Start a new bounded run with the budget in GOAL.md, retaining the previous run's results. Set a new start/deadline and task counter. Prioritize the three recorded next tasks unless GOAL.md changes their priority. Follow the same implementation and handoff protocol.
