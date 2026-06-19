---
workflow_version: 1
tracker_schema_version: 5
tracker:
    kind: tusker_vault
    active_states:
        - active
        - rework
    review_states:
        - review
    terminal_states:
        - done
        - cancelled
agents:
    default: codex
    enabled:
        - sarav
        - codex
        - claude-code
        - gemini
    max_concurrent_agents: 3
    max_concurrent_agents_by_state:
        rework: 1
runtime:
    poll_interval_ms: 5000
    lease_ttl_ms: 900000
    max_active_runs_per_project: 1
workspace:
    root: workspaces
    strategy: worktree
retry:
    max_attempts: 3
    backoff_ms:
        - 30000
        - 120000
        - 600000
reviewer:
    enabled: true
    runner: codex
    actor: agent-reviewer
    auto_close_risks:
        - low
        - medium
    human_required_risks:
        - high
        - critical
    prompt: |-
        You are the independent Tusker reviewer for {{ note.id }}.

        Review only. Do not edit implementation files. If the work needs changes, mark the task `rework` with a specific reason instead of fixing it yourself.

        Task:
        - ID: {{ note.id }}
        - Title: {{ note.title }}
        - Risk: {{ note.risk }}
        - Status: {{ note.status }}
        - Attempt: {{ attempt.id }}
        - Workspace: {{ workspace.path }}
        - Vault: {{ vault.path }}

        Policy:
        - Reviewer actor: {{ reviewer.actor }}
        - Auto-close allowed: {{ reviewer.auto_close_allowed }}
        - Human close required: {{ reviewer.human_required }}

        Checklist:
        1. Read the task acceptance contract, proof mode, verification, evidence, gates, and docs resolution.
        2. Inspect the current diff against the task scope. Call out surprise files or drive-by refactors.
        3. Run the verification commands needed to prove the acceptance contract.
        4. Confirm docs impact is applied, nooped, or waived for every `doc_nodes` entry.
        5. For risk high or critical, confirm the Knowledge delta is real and reviewer-actionable.
        6. If a caveat changes scope, decide whether it is acceptable or requires rework.

        If the task fails review, run:
        tusker status {{ note.id }} rework --by {{ reviewer.actor }} --reason "<specific unmet acceptance item>"

        If auto-close is allowed and every check passes, run:
        tusker docs check {{ note.id }}
        {{ reviewer.verify_command }}
        {{ reviewer.close_command }}

        If human close is required and every check passes, do not run `verify` or `close`. Leave the task in `review` and state the human-review recommendation in your final response.
codex:
    command: codex app-server
    approval_policy: on-request
    thread_sandbox: workspace-write
    turn_sandbox_policy: workspace-write
    turn_timeout_ms: 600000
    read_timeout_ms: 30000
    stall_timeout_ms: 120000
    max_turns: 1
claude:
    command: claude -p --output-format stream-json --input-format stream-json --permission-mode bypassPermissions
extensions:
    enabled: false
    allowed_tools: []
    allowed_mcps: []
    allow_tusker_read_tools: false
hooks:
    after_workspace_create: []
    before_workspace_remove: []
---

## Routing

You are working on {{ note.id }} for {{ project.name }}. Dispatch only makes sense because this task is currently {{ note.status }} and the workspace is ready at {{ workspace.path }}.

## Hard stop check

Before doing work, run `tusker closeout status {{ note.id }} --json` when the V7 closeout command is available. If it reports `agent_action=stop_until_human_response`, do not validate, inspect files, spawn subagents, or modify Tusker records. Reply with the pending human gates/proof and whether the closeout checkpoint or review packet is still needed.

Revalidate only after you edited files, a task/gate/evidence state changed, the closeout fingerprint no longer matches, or the user explicitly asked for fresh validation.

## Prompt

Use the installed Tusker skill bundle for durable task semantics and proof discipline. Work inside {{ workspace.path }}. Treat {{ repo.root }} as the source repository root for context only unless the task explicitly requires comparing against it.

Item: {{ note.title }}
Record: {{ note.record_id }}
Type: {{ note.type }}
Attempt: {{ attempt.number }}
Workflow: {{ workflow.path }}
Vault: {{ vault.path }}

## Command budget

Use the smallest command that proves or locates the next fact. Prefer packets/capsules, path-scoped status/search, repo-configured wrappers and build-lock/status commands, and redirected logs with small tails. Report validation as command + PASS/FAIL plus the first actionable failure; do not paste raw transcripts or repeat unchanged-state updates.

## Completion contract

Satisfy the task proof mode. For proof_mode=inline, record concise verification rows with `tusker verify add`; do not create evidence files. For card/artifact/audit, create only the evidence the proof mode requires. When machine work is complete and only human-owned proof or gates remain, run `tusker closeout <task-id> --emit-packet --validate "<command>"`, then stop. When the work is demonstrably ready for verification, use `tusker finish <task-id> --request-review` so the task reaches `review` or a branch-safe `propose status ... --status review` proposal is created. Attempt handoff alone is not a review request. If proof is blocked, create/propose a gate with a concrete owner, action, and verification instead of appending negative evidence.

## Reviewer contract

If `reviewer.enabled` is true, tasks in `review` may be dispatched to `reviewer.runner` for independent review. The reviewer must not edit implementation files. Low/medium risks can be verified and closed by `reviewer.actor` after all gates pass; high/critical risks stay in `review` for human verification and close.

## Retry policy

Retry only transient infrastructure failures. Human-directed rework creates a new active task revision.

## Human override policy

Humans may edit tasks directly, but runtime state belongs to the daemon store.
