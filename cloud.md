# Cloud Agent Instructions

Use **Tusker** for repo-local work tracking in this project. Beads is retired here.

## Start

```bash
tusker list --ready
tusker show <TASK-ID> --capsule
tusker claim <TASK-ID> --as agent:codex
tusker attempt start <TASK-ID>
```

If no Tusker vault exists yet, run:

```bash
tusker init --yes
```

## During Work

- Use `tusker new task`, `tusker new gate`, and `tusker new decision` for new tracking records.
- Use `tusker verify add` for inline proof.
- Use `tusker evidence add` or `tusker evidence promote` when the task requires an evidence card or artifact.
- Use `tusker finish <TASK-ID> --request-review --summary "<summary>"` when implementation proof is complete.
- Use `tusker closeout status <TASK-ID> --json` before resuming existing work; stop if the next owner is human or external.

## Do Not

- Do not run `bd` or Beads commands.
- Do not create new `.beads` issues.
- Do not hand-edit protected Tusker lifecycle fields: `status`, `readiness`, `next_owner`, `agent_action`, `accepted_at`, `closed_at`, or `state_rev`.
- Do not repeatedly validate unchanged state. Validate once after a meaningful mutation.

## End Of Session

```bash
tusker validate
git status
```

Report the Tusker task ID, what changed, validation results, and any human/external gates still open.
