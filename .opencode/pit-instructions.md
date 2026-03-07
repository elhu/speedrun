## Pit — Agent Instructions

You are an autonomous coding agent working on epic: qdm.

### CRITICAL: Single-Task Workflow Rules

**Work on ONE task at a time. No exceptions.**

1. **Pick ONE task only** - Never claim multiple tasks upfront
2. **Complete ALL steps** for a task before claiming the next one
3. **Never batch multiple tasks** into one commit
4. **Switch epics only when**:
   - Current epic is complete, OR
   - You're explicitly blocked and can't proceed
5. **Before starting a new epic**:
   - **MUST ask user for confirmation** before claiming any task from a different epic
   - Wait for explicit approval before proceeding

### Finding Work

- Run `bd ready --parent qdm` to find your next available ticket.
- If the command returns no tickets, all work for this epic is complete.

### Per-Task Workflow

**For EACH task, complete ALL steps before moving to the next:**

1. **Claim task**: `bd update <id> --status in_progress`
2. **Read task details**: `bd show <id>` - understand requirements fully
3. **Implement**: Write code, tests, documentation as specified
4. **Test locally**: Run affected tests to verify your changes work
5. **Review your code**: Check for issues, ensure quality
6. **Commit immediately**: Create ONE commit for this ONE task

   ```bash
   git add <files>
   git commit -m "Task summary

   Detailed description of what changed.

   Closes: <task-id>
   Co-Authored-By: Claude <noreply@anthropic.com>"
   ```

7. **Push immediately**: Sync work to remote after each task
   ```bash
   git pull --rebase
   bd sync
   git push
   ```
8. **Close task**: `bd close <id>`
9. **Report completion and wait**: Report the task is complete, committed, and pushed. WAIT for the user to tell you which task to pick next. DO NOT automatically claim the next task.

### Session Completion Protocol (Landing the Plane)

**When ending a work session**, you MUST complete ALL steps below:

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
   ```bash
   npm test           # Unit tests - must pass
   npm run test:e2e   # E2E tests - must pass (if available)
   npm run lint       # Linting - must pass
   ```
   **STOP if any test fails** - Fix before continuing
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Verify** - All changes committed AND pushed
6. **Hand off** - Provide context for next session

**CRITICAL: Work is NOT complete until `git push` succeeds.**

### Communication Rules

1. **Report progress clearly** at each step
2. **Ask for clarification** when task requirements are unclear
3. **Never assume or guess** implementation details
4. **Wait for explicit direction** before picking next task
5. **Never automatically pick the next task** after finishing one - always report completion and wait

### What NOT To Do

- **NEVER** claim 5+ tasks at once
- **NEVER** jump between epics without asking permission first
- **NEVER** work on "just one more quick task" without committing the previous one
- **NEVER** rationalize batching tasks as "efficient" - it makes code review impossible
- **NEVER** silently switch epics - always get user confirmation
- **NEVER** automatically pick the next task after finishing one
- **NEVER** commit without pushing immediately after - keeps work stranded locally

### Completion Protocol

When you finish working on a ticket:
1. Ensure all changes are committed and pushed
2. Close the ticket with `bd close <id>`
3. Stop working — the orchestrator will detect the closure and send you the next task

When there are no more tickets available (`bd ready --parent qdm` returns nothing):
- Close the last ticket and stop — the orchestrator will detect that no work remains

### Requesting Human Input

If you are stuck, unsure, or need a human decision:
1. Clearly explain what you need and why.
2. Wait for the orchestrator to send you a response.
